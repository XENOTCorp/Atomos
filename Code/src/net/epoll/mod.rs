//! HTTP/1.1 run-to-completion engine, built on the FDS transport engine
//! (`fds`): the epoll reactor is FDS's edge-triggered reactor with a
//! drain-to-EAGAIN busy-poll discipline, sockets are FDS's nonblocking
//! TCP transport (options applied before bind for SO_REUSEPORT group
//! admission), and per-connection state lives in FDS's preallocated
//! ConnTable with hot/cold cache-line halves and packed ConnectionId
//! tokens (a closed fd's number can never alias a live connection).
//! No tokio tasks, no spawn-per-conn. H2/H3 are not handled here.
//!
//! The receive loop allocates nothing after warm-up on the cached GET
//! path: HTTP `Conn` state is a slot array indexed by the packed
//! [`ConnectionId`] (not a `HashMap`), the request buffer is reserved
//! to 4 KiB at accept (a large POST may reserve once toward
//! `max_header+max_body`), consumed bytes are compacted once per
//! 64 KiB, and the encoder writes into a reused per-worker scratch.
//! Small responses write from the wire cache; leftovers park in a
//! 2 KiB `out` buffer. Byte-path bodies park in `out` up to
//! [`SF_MIN`]+headers. Bigger bodies go through `sendfile`. The hot
//! path does not realloc (ALLOC-01).

use std::io;
use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use fds::conn::{ConnTable, Connection, ConnectionId, CONN_CAP};
use fds::reactor::{EpollEvent, Interest, PollTimeout, Reactor};
use fds::tcp::TcpListener;
use fds::util::now_ticks;

use crate::access_log;
use crate::align::STATE_ON;
use crate::atom::AtomCtx;
use crate::encode::{append_chunk, append_chunk_end, encode_head, encode_response};
use crate::error::ServeError;
use crate::flags::FlagSet;
use crate::io::{Body, HeaderView, In, Out, OutBody};
use crate::parse::{
    decode_chunked_into, find_header_end, looks_like_json, parse_request, scan_json, ParseStatus,
};
use crate::status::Status;
use std::time::Instant;
use crate::pin_cpu;
use crate::cache::ResponseCache;
use crate::route::Router;
use crate::static_mod::SF_MIN;

mod conn;
mod tlsio;
mod write;
use conn::{Conn, PendingSf};
use write::flush_out;

/// Reserved token for the listener; connection tokens are packed
/// [`ConnectionId`]s (slot-based, so they never collide with the
/// listener token).
const TOKEN_LISTENER: u64 = u64::MAX;

/// Consumed bytes are compacted out of `Conn::buf` once this much has
/// been read (one memmove per 64 KiB instead of per request).
const COMPACT_THRESHOLD: usize = 64 * 1024;

/// Accept-time request buffer. Cached GET/HEAD stay here. A large POST
/// may reserve once toward `max_header+max_body` (ALLOC-04).
pub const IN_CAP: usize = 4096;

/// Encoded-header scratch and leftover-park cap. Cached GET responses
/// fit here. Byte-path file bodies may park up to [`out_max`].
pub const OUT_CAP: usize = 2048;

/// Max parked outbound bytes: one byte-path response (sendfile starts
/// at [`SF_MIN`]) plus header scratch.
pub const fn out_max() -> usize {
    SF_MIN as usize + OUT_CAP
}

/// Slot index for a live HTTP connection token. `None` for the listener
/// or an out-of-range id.
pub const fn http_slot(token: u64) -> Option<usize> {
    if token == TOKEN_LISTENER {
        return None;
    }
    let slot = ConnectionId::from_u64(token).slot() as usize;
    if slot < CONN_CAP {
        Some(slot)
    } else {
        None
    }
}

pub const fn buf_capacity_for(max_header: usize, max_body: usize) -> usize {
    max_header.saturating_add(max_body)
}

/// Append `src` only when it fits in existing capacity. Never realloc.
pub fn append_in_cap(buf: &mut Vec<u8>, src: &[u8]) -> bool {
    if src.len() > buf.capacity().saturating_sub(buf.len()) {
        return false;
    }
    buf.extend_from_slice(src);
    true
}

/// Copy `src` onto `out` only when it fits in existing capacity.
pub fn copy_into_out(out: &mut Vec<u8>, src: &[u8]) -> bool {
    append_in_cap(out, src)
}

/// Append toward a declared max. Reserves at most once when the body
/// exceeds the accept-time header reservation. Never grows past `max`.
fn append_bounded(buf: &mut Vec<u8>, src: &[u8], max: usize) -> bool {
    let need = buf.len().saturating_add(src.len());
    if need > max {
        return false;
    }
    if src.len() > buf.capacity().saturating_sub(buf.len()) {
        buf.reserve(src.len());
    }
    buf.extend_from_slice(src);
    true
}

pub fn run(router: Arc<Router>, ctx: Arc<AtomCtx>) -> Result<(), ServeError> {
    let mut addr: SocketAddr = router
        .cfg
        .bind
        .parse()
        .map_err(|_| ServeError::Config("bind".into()))?;
    if router.cfg.refuse_ports.contains(&addr.port()) {
        return Err(ServeError::Config(
            format!("bind port {} is in refuse_ports", addr.port()).into(),
        ));
    }
    let tcp_cfg = fds::config::TcpConfig {
        nodelay: router.cfg.tcp_nodelay,
        reuseport: router.cfg.so_reuseport,
        fastopen: if router.cfg.tcp_fastopen {
            router.cfg.backlog.max(1) as u32
        } else {
            0
        },
        ..Default::default()
    };
    let n = if tcp_cfg.reuseport {
        router.cfg.workers.max(1)
    } else {
        1
    };
    let mut tcps = Vec::with_capacity(n as usize);
    for i in 0..n {
        let l = TcpListener::bind(addr, &tcp_cfg, router.cfg.backlog)?;
        if i == 0 {
            addr = l.local_addr()?;
        }
        tcps.push(l);
    }
    crate::ops::jail::after_bind(&router.cfg)?;
    ctx.signal.v.store(STATE_ON, Ordering::Release);
    tracing::info!(local = %addr, workers = n, engine = "epoll", "atomos listen");

    let tls_cfg = if router.cfg.h1_tls {
        let ocsp = match &router.cfg.tls_ocsp {
            Some(p) => Some(std::fs::read(p)?),
            None => None,
        };
        Some(crate::tls::h1_only_server(
            router.cfg.tls_cert.as_deref(),
            router.cfg.tls_key.as_deref(),
            ocsp.as_deref(),
            router.cfg.tls_ticket_lifetime_secs,
        )?)
    } else {
        None
    };

    let mut joins = Vec::with_capacity(n as usize);
    for i in 0..n {
        let tcp = tcps.remove(0);
        let router = router.clone();
        let ctx = ctx.clone();
        let tls_cfg = tls_cfg.clone();
        let h = std::thread::Builder::new()
            .name(format!("atomos-epoll-{i}"))
            .spawn(move || {
                if router.cfg.cpu_pin {
                    let _ = pin_cpu::pin_to_cpu(i as usize);
                }
                if let Err(e) = worker(tcp, router, ctx, tls_cfg) {
                    tracing::debug!(%e, "epoll worker");
                }
            })
            .map_err(ServeError::Io)?;
        joins.push(h);
    }
    for h in joins {
        let _ = h.join();
    }
    Ok(())
}

fn worker(
    listener: TcpListener,
    router: Arc<Router>,
    ctx: Arc<AtomCtx>,
    tls_cfg: Option<Arc<rustls::ServerConfig>>,
) -> io::Result<()> {
    let mut reactor = Reactor::new(64)?;
    reactor.register(listener.as_raw_fd(), TOKEN_LISTENER, Interest::Readable)?;

    // Preallocated per-worker connection table (hot/cold halves; packed
    // slot tokens). HTTP Conn state is a slot array indexed by the
    // token's low half. A closed fd's number never aliases a live slot.
    let conns: ConnTable<CONN_CAP> = ConnTable::new();
    let now = now_ticks();
    for i in 0..CONN_CAP {
        conns.initialize(i, Connection::new("0.0.0.0:0".parse().unwrap(), now));
    }
    let mut streams: Vec<Option<Conn<'_>>> = (0..CONN_CAP).map(|_| None).collect();
    let mut enc = Vec::with_capacity(OUT_CAP);
    let mut listener = Some(listener);
    let mut drain_since: Option<Instant> = None;

    // 200 ms poll timeout doubles as the stop-poll cadence (shutdown
    // latency <= 200 ms), matching the pre-FDS engine.
    let timeout = PollTimeout {
        tv_sec: 0,
        tv_nsec: 200_000_000,
    };
    let mut evbuf = vec![EpollEvent::default(); 64];
    loop {
        if ctx.stop.v.load(Ordering::Acquire) != 0 {
            break;
        }
        if ctx.drain.v.load(Ordering::Acquire) != 0 {
            if drain_since.is_none() {
                drain_since = Some(Instant::now());
                if let Some(l) = listener.take() {
                    let _ = reactor.unregister(l.as_raw_fd());
                    drop(l);
                }
            }
            let wait_ms = router.cfg.worker_shutdown_timeout_ms.max(1);
            let expired = drain_since
                .map(|t| t.elapsed().as_millis() as u64 >= wait_ms)
                .unwrap_or(false);
            let empty = streams.iter().all(|s| s.is_none());
            if expired || empty {
                for slot in streams.iter_mut() {
                    if let Some(c) = slot.take() {
                        let _ = reactor.unregister(c.stream.as_raw_fd());
                    }
                }
                ctx.stop.v.store(1, Ordering::Release);
                break;
            }
        }
        let n = match reactor.poll_timeout(Some(&timeout)) {
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        reap_idle(&mut reactor, &mut streams, &router, &mut enc);
        if n == 0 {
            continue;
        }
        let m = reactor.copy_events(n, &mut evbuf);
        for ev in evbuf.iter().take(m) {
            let token = ev.token;
            if token == TOKEN_LISTENER {
                if let Some(l) = listener.as_ref() {
                    accept_loop(l, &mut reactor, &conns, &mut streams, tls_cfg.as_ref());
                }
                continue;
            }
            let Some(idx) = http_slot(token) else {
                continue;
            };
            let mut drop_fd = ev.error || ev.hang_up;
            if let Some(c) = streams[idx].as_mut() {
                // A pending sendfile (or buffered out bytes) means the
                // response is mid-write: do not read/parse the next
                // pipelined request until it is fully sent.
                if !drop_fd && ev.readable && c.out.is_empty() && c.pending_sf.is_none() {
                    match read_and_serve(c, &router, &mut enc) {
                        Ok(false) => drop_fd = true,
                        Ok(true) => {}
                        Err(_) => drop_fd = true,
                    }
                }
                let busy = !c.out.is_empty() || c.pending_sf.is_some() || tlsio::wants_write(c);
                if busy && (ev.writable || ev.readable) {
                    if flush_out(c).is_err() {
                        drop_fd = true;
                    } else if c.out.is_empty() && c.pending_sf.is_none() && !tlsio::wants_write(c)
                    {
                        let _ = reactor.modify(c.stream.as_raw_fd(), token, Interest::Readable);
                    } else {
                        let _ =
                            reactor.modify(c.stream.as_raw_fd(), token, Interest::ReadableWritable);
                    }
                } else if busy {
                    let _ = reactor.modify(c.stream.as_raw_fd(), token, Interest::ReadableWritable);
                }
            }
            if drop_fd {
                // Taking the Conn drops the slot guard, which releases
                // the table slot exactly once.
                if let Some(c) = streams[idx].take() {
                    let _ = reactor.unregister(c.stream.as_raw_fd());
                }
            }
        }
    }
    Ok(())
}

fn accept_loop<'a>(
    listener: &TcpListener,
    reactor: &mut Reactor,
    conns: &'a ConnTable<CONN_CAP>,
    streams: &mut [Option<Conn<'a>>],
    tls_cfg: Option<&Arc<rustls::ServerConfig>>,
) {
    loop {
        match listener.accept() {
            Ok(Some((stream, peer))) => {
                let Some(mut slot) = conns.try_acquire() else {
                    continue;
                };
                let idx = slot.index();
                let conn = slot.conn_mut();
                conn.cold.peer = peer;
                conn.cold.established_at = now_ticks();
                let token = ConnectionId::new(0, idx as u32).as_u64();
                if reactor
                    .register(stream.as_raw_fd(), token, Interest::Readable)
                    .is_err()
                {
                    continue; // slot guard drops -> slot released
                }
                let tls = match tls_cfg {
                    Some(cfg) => match rustls::ServerConnection::new(cfg.clone()) {
                        Ok(c) => Some(Box::new(c)),
                        Err(_) => continue,
                    },
                    None => None,
                };
                // 4 KiB at accept. GET/HEAD never touch max_body. A large
                // POST may reserve once toward buf_cap (ALLOC-04).
                streams[idx] = Some(Conn {
                    stream,
                    peer,
                    buf: Vec::with_capacity(IN_CAP),
                    pos: 0,
                    out: Vec::with_capacity(OUT_CAP),
                    out_off: 0,
                    pending_sf: None,
                    last_rw: Instant::now(),
                    hdr_t0: None,
                    served: false,
                    tls,
                    slot,
                });
            }
            Ok(None) => break,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
            Err(_) => break,
        }
    }
}

fn park_out(c: &mut Conn<'_>, bytes: &[u8]) -> io::Result<()> {
    if copy_into_out(&mut c.out, bytes) {
        return Ok(());
    }
    let need = c.out.len().saturating_add(bytes.len());
    if need > out_max() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "epoll: out cap (ALLOC-01)",
        ));
    }
    c.out.reserve(bytes.len());
    c.out.extend_from_slice(bytes);
    Ok(())
}

fn queue_bytes(c: &mut Conn<'_>, bytes: &[u8]) -> io::Result<()> {
    // Write first when `out` is empty (cached GET: one send, no extra
    // copy). Park into `out` only for a leftover or a queued tail.
    if !c.out.is_empty() {
        if copy_into_out(&mut c.out, bytes) {
            return Ok(());
        }
        flush_out(c)?;
        if copy_into_out(&mut c.out, bytes) {
            return Ok(());
        }
        if !c.out.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "epoll: out cap (ALLOC-01)",
            ));
        }
    }
    match tlsio::write_plain(c, bytes) {
        Ok(n) if n == bytes.len() => Ok(()),
        Ok(n) => park_out(c, &bytes[n..]),
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => park_out(c, bytes),
        Err(e) => Err(e),
    }
}

fn read_and_serve(c: &mut Conn<'_>, router: &Router, enc: &mut Vec<u8>) -> io::Result<bool> {
    let mut tmp = [0u8; 4096];
    let mut eof = false;
    loop {
        match tlsio::read_plain(c, &mut tmp) {
            Ok(None) => break,
            Ok(Some(0)) => {
                if c.tls.as_ref().is_some_and(|t| t.is_handshaking()) {
                    return Ok(true);
                }
                eof = true;
                break;
            }
            Ok(Some(n)) => {
                c.last_rw = Instant::now();
                if c.hdr_t0.is_none() {
                    c.hdr_t0 = Some(c.last_rw);
                }
                let max = buf_capacity_for(
                    router.cfg.max_header_bytes,
                    router.cfg.max_body_bytes,
                );
                if !append_bounded(&mut c.buf, &tmp[..n], max) {
                    return Ok(false);
                }
                // Hot state: sequence + activity on every step (the
                // FDS hot/cold split in action).
                let hot = &mut c.slot.conn_mut().hot;
                hot.seq = hot.seq.wrapping_add(n as u32);
                hot.last_activity = now_ticks();
            }
            Err(_) => return Ok(false),
        }
        if c.buf.len() - c.pos > router.cfg.max_header_bytes + router.cfg.max_body_bytes {
            return Ok(false);
        }
    }
    loop {
        // Compact consumed bytes once per 64 KiB (amortized memmove).
        if c.pos >= COMPACT_THRESHOLD {
            c.buf.drain(..c.pos);
            c.pos = 0;
        }
        match parse_request(&c.buf[c.pos..], router.cfg.max_header_bytes) {
            Ok(ParseStatus::Partial) => return Ok(!eof),
            Err(_) => {
                reply_status(c, enc, Status::BAD_REQUEST)?;
                return Ok(false);
            }
            Ok(ParseStatus::Complete(p)) => {
                if p.content_length > router.cfg.max_body_bytes {
                    reply_status(c, enc, Status::from_u16(413))?;
                    return Ok(false);
                }
                let need_rel = p.wire_end;
                if c.buf.len() - c.pos < need_rel {
                    return Ok(true);
                }
                let need = c.pos + need_rel;
                if c.buf.get(c.pos) == Some(&b'P') && c.buf[c.pos..].starts_with(b"PRI ") {
                    return Ok(false);
                }
                if p.upgrade {
                    reply_status(c, enc, Status::UPGRADE_REQUIRED)?;
                    return Ok(false);
                }
                let head = p.method == crate::io::Method::Head;
                if p.content_length == 0 {
                    if let Some(hit) = router.cache.get(p.method, p.path, p.query) {
                        if ResponseCache::not_modified(&p.headers, &hit) {
                            let o = Out::empty(Status::NOT_MODIFIED);
                            enc.clear();
                            encode_response(&o, enc);
                            let ka = p.keepalive;
                            c.pos = need;
                            c.served = true;
                            c.hdr_t0 = None;
                            queue_bytes(c, enc)?;
                            if !ka {
                                let _ = flush_out(c);
                                return Ok(false);
                            }
                            continue;
                        }
                        if head {
                            enc.clear();
                            encode_head(&hit, enc);
                            let ka = p.keepalive;
                            c.pos = need;
                            c.served = true;
                            c.hdr_t0 = None;
                            queue_bytes(c, enc)?;
                            if !ka {
                                let _ = flush_out(c);
                                return Ok(false);
                            }
                            continue;
                        }
                        if let Some(wire) = router.cache.get_wire(p.method, p.path, p.query) {
                            let ka = p.keepalive;
                            if router.cfg.access_log {
                                access_log::emit(
                                    p.method,
                                    p.path,
                                    hit.status.as_u16(),
                                    hit.body.len(),
                                );
                            }
                            c.pos = need;
                            c.served = true;
                            c.hdr_t0 = None;
                            queue_bytes(c, wire.as_ref())?;
                            if !ka {
                                let _ = flush_out(c);
                                return Ok(false);
                            }
                            continue;
                        }
                    }
                }
                let mut decoded = Vec::new();
                let body_bytes: &[u8] = if p.chunked {
                    let start = c.pos + p.header_end;
                    decode_chunked_into(&c.buf[start..need], &mut decoded)
                        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "chunked"))?;
                    &decoded
                } else {
                    &c.buf[c.pos + p.header_end..need]
                };
                if looks_like_json(body_bytes)
                    && scan_json(body_bytes, router.cfg.max_json_depth).is_err()
                {
                    reply_status(c, enc, Status::BAD_REQUEST)?;
                    return Ok(false);
                }
                let body = if body_bytes.is_empty() {
                    Body::Empty
                } else if looks_like_json(body_bytes) {
                    Body::Json(body_bytes)
                } else {
                    Body::Raw(body_bytes)
                };
                let req = In {
                    method: p.method,
                    path: p.path,
                    query: p.query,
                    headers: HeaderView { pairs: p.headers },
                    body,
                    peer: c.peer,
                    flags: FlagSet::empty(),
                };
                let ka = p.keepalive;
                let log = router
                    .cfg
                    .access_log
                    .then(|| (p.method, p.path.to_string()));
                let t0 = Instant::now();
                let mut out = router.dispatch(req);
                if t0.elapsed().as_millis() as u64 > router.cfg.module_timeout_ms.max(1) {
                    out = Out::empty(Status::GATEWAY_TIMEOUT);
                }
                enc.clear();
                if head {
                    encode_head(&out, enc);
                    queue_bytes(c, enc)?;
                } else if matches!(out.body, OutBody::Stream(_)) {
                    encode_response(&out, enc);
                    queue_bytes(c, enc)?;
                    if let OutBody::Stream(s) = &out.body {
                        let mut rx = s.take();
                        while let Ok(chunk) = rx.try_recv() {
                            enc.clear();
                            append_chunk(enc, &chunk);
                            queue_bytes(c, enc)?;
                        }
                    }
                    enc.clear();
                    append_chunk_end(enc);
                    queue_bytes(c, enc)?;
                } else {
                    encode_response(&out, enc);
                    queue_bytes(c, enc)?;
                    if let OutBody::File(f) = &out.body {
                        c.pending_sf = Some(PendingSf {
                            file: f.file.clone(),
                            offset: f.offset as libc::off_t,
                            len: f.len,
                        });
                    }
                }
                if let Some((method, path)) = log {
                    access_log::emit(method, &path, out.status.as_u16(), out.body.len());
                }
                c.pos = need;
                c.served = true;
                c.hdr_t0 = None;
                if !ka {
                    let _ = flush_out(c);
                    return Ok(false);
                }
            }
        }
    }
}

fn reply_status(c: &mut Conn<'_>, enc: &mut Vec<u8>, st: Status) -> io::Result<()> {
    let out = Out::empty(st);
    enc.clear();
    encode_response(&out, enc);
    queue_bytes(c, enc)?;
    let _ = flush_out(c);
    Ok(())
}

fn reap_idle(
    reactor: &mut Reactor,
    streams: &mut [Option<Conn<'_>>],
    router: &Router,
    enc: &mut Vec<u8>,
) {
    let now = Instant::now();
    for slot in streams.iter_mut() {
        let Some(c) = slot.as_ref() else {
            continue;
        };
        let pending = c.buf.len() > c.pos;
        let headers_done = pending && find_header_end(&c.buf[c.pos..]).is_some();
        let (t0, limit, body_to) = if !pending {
            // New conn with no bytes: header budget. Keep-alive idle
            // after a served request: idle budget.
            let limit = if c.served {
                router.cfg.idle_timeout_ms
            } else {
                router.cfg.header_timeout_ms
            };
            (c.last_rw, limit, false)
        } else if !headers_done {
            (c.hdr_t0.unwrap_or(c.last_rw), router.cfg.header_timeout_ms, false)
        } else {
            (c.last_rw, router.cfg.body_timeout_ms, true)
        };
        let idle_ms = now.saturating_duration_since(t0).as_millis() as u64;
        if idle_ms <= limit.max(1) {
            continue;
        }
        if body_to {
            if let Some(c) = slot.as_mut() {
                let _ = reply_status(c, enc, Status::REQUEST_TIMEOUT);
            }
        }
        if let Some(c) = slot.take() {
            let _ = reactor.unregister(c.stream.as_raw_fd());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoll_run_is_blocking_signature() {
        let _: fn(Arc<Router>, Arc<AtomCtx>) -> Result<(), ServeError> = run;
    }

    #[test]
    fn connection_id_tokens_never_collide_with_listener() {
        // Connection tokens are packed slot ids in the low half; the
        // listener token is u64::MAX. No overlap is possible.
        assert_ne!(ConnectionId::new(0, u32::MAX).as_u64(), TOKEN_LISTENER);
        assert_eq!(ConnectionId::new(0, 7).as_u64() & !0xFFFF_FFFF, 0);
    }

    #[test]
    fn http_slot_is_connection_id_low_half() {
        let tok = ConnectionId::new(0, 7).as_u64();
        assert_eq!(http_slot(tok), Some(7));
        assert_eq!(http_slot(TOKEN_LISTENER), None);
        assert_eq!(
            http_slot(ConnectionId::new(0, CONN_CAP as u32).as_u64()),
            None
        );
    }

    #[test]
    fn buf_capacity_covers_header_and_body() {
        assert_eq!(buf_capacity_for(16, 32), 48);
    }

    #[test]
    fn append_in_cap_does_not_grow() {
        let mut b = Vec::with_capacity(16);
        assert!(append_in_cap(&mut b, b"hello"));
        assert_eq!(b.capacity(), 16);
        assert_eq!(&b, b"hello");
        assert!(!append_in_cap(&mut b, &[0u8; 32]));
        assert_eq!(b.capacity(), 16);
        assert_eq!(&b, b"hello");
    }

    #[test]
    fn copy_into_out_does_not_grow() {
        let mut out = Vec::with_capacity(8);
        assert!(copy_into_out(&mut out, b"abcd"));
        assert_eq!(out.capacity(), 8);
        assert!(!copy_into_out(&mut out, b"12345"));
        assert_eq!(out.capacity(), 8);
        assert_eq!(&out, b"abcd");
    }

    #[test]
    fn encode_scratch_fits_small_json_without_grow() {
        use crate::flags::FlagSet;
        use crate::io::{CacheDirective, Out, OutBody};
        use crate::status::Status;
        use bytes::Bytes;

        let out = Out {
            status: Status::OK,
            reason: None,
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: OutBody::Json(Bytes::from_static(br#"{"ok":true}"#)),
            cache: CacheDirective::No,
            flags: FlagSet::empty(),
        };
        let mut enc = Vec::with_capacity(OUT_CAP);
        let cap = enc.capacity();
        encode_response(&out, &mut enc);
        assert_eq!(enc.capacity(), cap);
        assert!(!enc.is_empty());
        assert!(enc.len() <= OUT_CAP);
    }

    #[test]
    fn out_max_covers_byte_path_file() {
        assert_eq!(out_max(), SF_MIN as usize + OUT_CAP);
        assert!(out_max() > 64 * 1024);
    }

    #[test]
    fn accept_reserve_is_4k() {
        assert_eq!(IN_CAP, 4096);
        const { assert!(IN_CAP < 16 * 1024); }
    }
}
