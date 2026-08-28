//! HTTP/1.1 run-to-completion engine, built on the FDS transport engine
//! (`fds`): the epoll reactor is FDS's edge-triggered reactor with a
//! drain-to-EAGAIN busy-poll discipline, sockets are FDS's nonblocking
//! TCP transport (options applied before bind for SO_REUSEPORT group
//! admission), and per-connection state lives in FDS's preallocated
//! ConnTable with hot/cold cache-line halves and packed ConnectionId
//! tokens (a closed fd's number can never alias a live connection).
//! No tokio tasks, no spawn-per-conn. H2/H3 are not handled here.
//!
//! The receive loop allocates nothing after warm-up: HTTP `Conn` state
//! is a slot array indexed by the packed [`ConnectionId`] (not a
//! `HashMap`), the request buffer is pre-sized at accept to
//! `max_header + max_body` and never grown, consumed bytes are compacted
//! once per 64 KiB instead of a `drain` memmove per request, and the
//! response encoder writes into a reused per-worker scratch. Outgoing
//! bytes copy into a fixed `out` buffer or `write_all`; the hot path
//! does not realloc (ALLOC-01).

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
use crate::encode::encode_response;
use crate::error::ServeError;
use crate::flags::FlagSet;
use crate::io::{Body, HeaderView, In, OutBody};
use crate::parse::{looks_like_json, parse_request, scan_json, ParseStatus};
use crate::pin_cpu;
use crate::route::Router;

mod conn;
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

/// Encoded-header scratch and cache-hit copy cap. Sized on the worker
/// (control path). The hot path never grows these.
pub const OUT_CAP: usize = 2048;

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

    let mut joins = Vec::with_capacity(n as usize);
    for i in 0..n {
        let tcp = tcps.remove(0);
        let router = router.clone();
        let ctx = ctx.clone();
        let h = std::thread::Builder::new()
            .name(format!("atomos-epoll-{i}"))
            .spawn(move || {
                if router.cfg.cpu_pin {
                    let _ = pin_cpu::pin_to_cpu(i as usize);
                }
                if let Err(e) = worker(tcp, router, ctx) {
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

fn worker(listener: TcpListener, router: Arc<Router>, ctx: Arc<AtomCtx>) -> io::Result<()> {
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
        let n = match reactor.poll_timeout(Some(&timeout)) {
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if n == 0 {
            continue;
        }
        let m = reactor.copy_events(n, &mut evbuf);
        for ev in evbuf.iter().take(m) {
            let token = ev.token;
            if token == TOKEN_LISTENER {
                accept_loop(&listener, &mut reactor, &conns, &mut streams, &router);
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
                let busy = !c.out.is_empty() || c.pending_sf.is_some();
                if busy && (ev.writable || ev.readable) {
                    if flush_out(c).is_err() {
                        drop_fd = true;
                    } else if c.out.is_empty() && c.pending_sf.is_none() {
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
    router: &Router,
) {
    let buf_cap = buf_capacity_for(router.cfg.max_header_bytes, router.cfg.max_body_bytes);
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
                streams[idx] = Some(Conn {
                    stream,
                    peer,
                    buf: Vec::with_capacity(buf_cap),
                    pos: 0,
                    out: Vec::with_capacity(OUT_CAP),
                    out_off: 0,
                    pending_sf: None,
                    slot,
                });
            }
            Ok(None) => break,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
            Err(_) => break,
        }
    }
}

fn queue_bytes(c: &mut Conn<'_>, bytes: &[u8]) -> io::Result<()> {
    if copy_into_out(&mut c.out, bytes) {
        return Ok(());
    }
    // Does not fit. Flush any already-queued bytes first so a
    // write_all of `bytes` cannot overtake them (pipelined responses).
    if !c.out.is_empty() {
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
    c.stream.write_all(bytes)
}

fn read_and_serve(c: &mut Conn<'_>, router: &Router, enc: &mut Vec<u8>) -> io::Result<bool> {
    let mut tmp = [0u8; 4096];
    loop {
        match c.stream.read(&mut tmp) {
            Ok(0) => return Ok(false),
            Ok(n) => {
                if !append_in_cap(&mut c.buf, &tmp[..n]) {
                    return Ok(false);
                }
                // Hot state: sequence + activity on every step (the
                // FDS hot/cold split in action).
                let hot = &mut c.slot.conn_mut().hot;
                hot.seq = hot.seq.wrapping_add(n as u32);
                hot.last_activity = now_ticks();
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
            Err(e) => return Err(e),
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
            Ok(ParseStatus::Partial) => return Ok(true),
            Err(_) => return Ok(false),
            Ok(ParseStatus::Complete(p)) => {
                if p.content_length > router.cfg.max_body_bytes {
                    return Ok(false);
                }
                let need_rel = p.header_end + p.content_length;
                if c.buf.len() - c.pos < need_rel {
                    return Ok(true);
                }
                let need = c.pos + need_rel;
                if c.buf.get(c.pos) == Some(&b'P') && c.buf[c.pos..].starts_with(b"PRI ") {
                    return Ok(false);
                }
                if p.content_length == 0 {
                    if let Some(wire) = router.cache.get_wire(p.method, p.path, p.query) {
                        let ka = p.keepalive;
                        if router.cfg.access_log {
                            // Prefer Out body len when still cached; else skip bytes.
                            let (status, blen) = match router.cache.get(p.method, p.path, p.query) {
                                Some(o) => (o.status.as_u16(), o.body.len()),
                                None => (200, 0),
                            };
                            access_log::emit(p.method, p.path, status, blen);
                        }
                        c.pos = need;
                        queue_bytes(c, wire.as_ref())?;
                        if !ka {
                            let _ = flush_out(c);
                            return Ok(false);
                        }
                        continue;
                    }
                }
                let body_bytes = &c.buf[c.pos + p.header_end..need];
                if looks_like_json(body_bytes)
                    && scan_json(body_bytes, router.cfg.max_json_depth).is_err()
                {
                    let ka = p.keepalive;
                    c.pos = need;
                    return Ok(ka);
                }
                let body = if p.content_length == 0 {
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
                let out = router.dispatch(req);
                // Encode into the reused per-worker scratch: no heap
                // allocation per request after warm-up (ALLOC-01).
                enc.clear();
                encode_response(&out, enc);
                queue_bytes(c, enc)?;
                if let OutBody::File(f) = &out.body {
                    // Headers are in `out` (Content-Length = file size);
                    // the body goes kernel-side via sendfile.
                    c.pending_sf = Some(PendingSf {
                        file: f.file.clone(),
                        offset: f.offset as libc::off_t,
                        len: f.len,
                    });
                }
                if let Some((method, path)) = log {
                    access_log::emit(method, &path, out.status.as_u16(), out.body.len());
                }
                c.pos = need;
                if !ka {
                    let _ = flush_out(c);
                    return Ok(false);
                }
            }
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
}
