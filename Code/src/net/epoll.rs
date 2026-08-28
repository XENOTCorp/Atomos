//! HTTP/1.1 run-to-completion engine, built on the FDS transport engine
//! (`fds`): the epoll reactor is FDS's edge-triggered reactor with a
//! drain-to-EAGAIN busy-poll discipline, sockets are FDS's nonblocking
//! TCP transport (options applied before bind for SO_REUSEPORT group
//! admission), and per-connection state lives in FDS's preallocated
//! ConnTable with hot/cold cache-line halves and packed ConnectionId
//! tokens (a closed fd's number can never alias a live connection).
//! No tokio tasks, no spawn-per-conn. H2/H3 are not handled here.
//!
//! The receive loop allocates nothing after warm-up: the request buffer
//! is consumed through a read cursor (compacted once per 64 KiB instead
//! of a `drain` memmove per request) and the response encoder writes
//! into a reused thread-local scratch instead of a fresh `Vec` per
//! request.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::os::fd::AsRawFd;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use fds::conn::{ConnTable, Connection, ConnectionId, ConnectionSlot, CONN_CAP};
use fds::reactor::{EpollEvent, Interest, PollTimeout, Reactor};
use fds::tcp::{TcpListener, TcpStream};
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

/// Reserved token for the listener; connection tokens are packed
/// [`ConnectionId`]s (slot-based, so they never collide with the
/// listener token).
const TOKEN_LISTENER: u64 = u64::MAX;

/// Consumed bytes are compacted out of `Conn::buf` once this much has
/// been read (one memmove per 64 KiB instead of per request).
const COMPACT_THRESHOLD: usize = 64 * 1024;

// Reused response-encoding scratch (per worker thread; zero allocation
// per request after warm-up).
thread_local! {
    static ENC: RefCell<Vec<u8>> = RefCell::new(Vec::with_capacity(2048));
}

struct Conn<'a> {
    stream: TcpStream,
    peer: SocketAddr,
    buf: Vec<u8>,
    /// Bytes of `buf` already consumed by completed requests (read
    /// cursor; compacted out past [`COMPACT_THRESHOLD`]).
    pos: usize,
    out: Vec<u8>,
    out_off: usize,
    /// File body being sent with `sendfile` after the header bytes in
    /// `out` are flushed. Non-None means the response is not fully
    /// written: the worker keeps writable interest and skips reads
    /// until it completes (request/response order must be preserved).
    pending_sf: Option<PendingSf>,
    /// Held for the connection's lifetime: dropping it releases the
    /// table slot exactly once (never call `release_slot` while a guard
    /// is alive — that would double-release the free-list ring).
    slot: ConnectionSlot<'a, CONN_CAP>,
}

/// Remaining range of an open file to send kernel-side.
struct PendingSf {
    file: Arc<std::fs::File>,
    /// `off_t` so it can be passed to `sendfile` without a cast.
    offset: libc::off_t,
    len: u64,
}

/// Blocking. One pinned OS thread per worker; no tokio on the datapath.
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
    // slot tokens). The HTTP Conn state (stream, buf, out) lives in a
    // HashMap keyed by the token, so a closed fd's number can never
    // alias a live connection.
    let conns: ConnTable<CONN_CAP> = ConnTable::new();
    let now = now_ticks();
    for i in 0..CONN_CAP {
        conns.initialize(i, Connection::new("0.0.0.0:0".parse().unwrap(), now));
    }
    let mut streams: HashMap<u64, Conn<'_>> = HashMap::new();

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
                accept_loop(&listener, &mut reactor, &conns, &mut streams);
                continue;
            }
            let mut drop_fd = ev.error || ev.hang_up;
            if let Some(c) = streams.get_mut(&token) {
                // A pending sendfile (or buffered out bytes) means the
                // response is mid-write: do not read/parse the next
                // pipelined request until it is fully sent.
                if !drop_fd && ev.readable && c.out.is_empty() && c.pending_sf.is_none() {
                    match read_and_serve(c, &router) {
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
                        let _ = reactor.modify(c.stream.as_raw_fd(), token, Interest::ReadableWritable);
                    }
                } else if busy {
                    let _ = reactor.modify(c.stream.as_raw_fd(), token, Interest::ReadableWritable);
                }
            }
            if drop_fd {
                // Removing the Conn drops the slot guard, which releases
                // the table slot exactly once.
                if let Some(c) = streams.remove(&token) {
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
    streams: &mut HashMap<u64, Conn<'a>>,
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
                streams.insert(
                    token,
                    Conn {
                        stream,
                        peer,
                        buf: Vec::with_capacity(4096),
                        pos: 0,
                        out: Vec::new(),
                        out_off: 0,
                        pending_sf: None,
                        slot,
                    },
                );
            }
            Ok(None) => break,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
            Err(_) => break,
        }
    }
}

fn read_and_serve(c: &mut Conn<'_>, router: &Router) -> io::Result<bool> {
    let mut tmp = [0u8; 4096];
    loop {
        match c.stream.read(&mut tmp) {
            Ok(0) => return Ok(false),
            Ok(n) => {
                c.buf.extend_from_slice(&tmp[..n]);
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
                            let (status, blen) =
                                match router.cache.get(p.method, p.path, p.query) {
                                    Some(o) => (o.status.as_u16(), o.body.len()),
                                    None => (200, 0),
                                };
                            access_log::emit(p.method, p.path, status, blen);
                        }
                        c.pos = need;
                        c.out.extend_from_slice(wire.as_ref());
                        c.out_off = 0;
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
                let log = router.cfg.access_log.then(|| (p.method, p.path.to_string()));
                let out = router.dispatch(req);
                // Encode into the reused thread-local scratch: no heap
                // allocation per request after warm-up.
                ENC.with(|enc| {
                    let mut enc = enc.borrow_mut();
                    enc.clear();
                    encode_response(&out, &mut enc);
                    c.out.extend_from_slice(&enc);
                });
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

fn flush_out(c: &mut Conn<'_>) -> io::Result<()> {
    while c.out_off < c.out.len() {
        match c.stream.write_all(&c.out[c.out_off..]) {
            Ok(()) => c.out_off = c.out.len(),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(()),
            Err(e) => return Err(e),
        }
    }
    c.out.clear();
    c.out_off = 0;
    // Drain the file body kernel-side. Take/put so the borrow checker
    // does not hold `pending_sf` while `stream` is borrowed.
    loop {
        let mut sf = match c.pending_sf.take() {
            Some(s) => s,
            None => break,
        };
        // SAFETY: both fds are valid and owned by this connection; the
        // offset/count stay within the file range the fd was opened
        // for (StaticMod sets offset=0, len=file size).
        let n = unsafe {
            libc::sendfile(
                c.stream.as_raw_fd(),
                sf.file.as_raw_fd(),
                &mut sf.offset as *mut libc::off_t,
                sf.len as usize,
            )
        };
        if n < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::WouldBlock {
                c.pending_sf = Some(sf);
                return Ok(());
            }
            return Err(e);
        }
        let n = n as u64;
        if n == 0 {
            // EOF before the range was sent: the file shrank underneath
            // us (cache is stale). Bail out rather than hang.
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "sendfile: file shorter than cached range",
            ));
        }
        if n >= sf.len {
            // Range fully sent.
            continue;
        }
        sf.offset += n as libc::off_t;
        sf.len -= n;
        c.pending_sf = Some(sf);
    }
    Ok(())
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
}
