//! Listener setup, worker readiness, slot ownership, draining and timeouts.
use super::conn::Conn;
use super::request::{drive_connection, reply_status};
use super::{http_slot, IN_CAP, OUT_CAP, TOKEN_LISTENER};
use crate::align::STATE_ON;
use crate::atom::AtomCtx;
use crate::error::ServeError;
use crate::parse::find_header_end;
use crate::pin_cpu;
use crate::route::Router;
use crate::status::Status;
use fds::conn::{ConnTable, Connection, ConnectionId, CONN_CAP};
use fds::reactor::{EpollEvent, Interest, PollTimeout, Reactor};
use fds::tcp::TcpListener;
use fds::util::now_ticks;
use std::io;
use std::net::SocketAddr;
use std::sync::{atomic::Ordering, Arc};
use std::time::Instant;

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
    for (i, tcp) in tcps.into_iter().enumerate() {
        let router = router.clone();
        let ctx = ctx.clone();
        let tls_cfg = tls_cfg.clone();
        let h = std::thread::Builder::new()
            .name(format!("atomos-epoll-{i}"))
            .spawn(move || {
                if router.cfg.cpu_pin {
                    let _ = pin_cpu::pin_to_cpu(i);
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
    let mut streams: Vec<Option<Conn<'_>>> = (0..CONN_CAP).map(|_| None).collect();
    let mut generations = vec![0u32; CONN_CAP];
    let mut enc = Vec::with_capacity(OUT_CAP);
    let mut listener = Some(listener);
    let mut drain_since: Option<Instant> = None;
    let mut last_reap = Instant::now();

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
        // Origin reaped only on poll timeout. Under load poll never
        // times out, so Slowloris would stick. Cadence matches the
        // 200 ms poll: do not walk CONN_CAP on every request.
        if n == 0 || last_reap.elapsed().as_millis() >= 200 {
            reap_idle(&mut reactor, &mut streams, &router, &mut enc);
            last_reap = Instant::now();
        }
        if n == 0 {
            continue;
        }
        let m = reactor.copy_events(n, &mut evbuf);
        for ev in evbuf.iter().take(m) {
            let token = ev.token;
            if token == TOKEN_LISTENER {
                if let Some(l) = listener.as_ref() {
                    accept_loop(
                        l,
                        &mut reactor,
                        &conns,
                        &mut streams,
                        &mut generations,
                        tls_cfg.as_ref(),
                    );
                }
                continue;
            }
            let Some(idx) = http_slot(token) else {
                continue;
            };
            let mut drop_fd = ev.error || ev.hang_up;
            if let Some(c) = streams[idx].as_mut() {
                if c.token != token {
                    continue;
                }
                let was_pending = c.has_pending_output();
                if !drop_fd {
                    drop_fd = !drive_connection(c, &router, &mut enc, ev.readable, ev.writable)
                        .unwrap_or(false);
                }
                // Cached small responses need no epoll_ctl syscall. Change
                // interest only when output is parked or has just drained.
                if !drop_fd && (was_pending || c.has_pending_output()) {
                    let interest = if c.has_pending_output() {
                        Interest::ReadableWritable
                    } else {
                        Interest::Readable
                    };
                    if reactor
                        .modify(c.stream.as_raw_fd(), token, interest)
                        .is_err()
                    {
                        drop_fd = true;
                    }
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
    generations: &mut [u32],
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
                *conn = Connection::new(peer, now_ticks());
                conn.hot.fd = stream.as_raw_fd();
                generations[idx] = generations[idx].wrapping_add(1);
                let token = ConnectionId::new(generations[idx], idx as u32).as_u64();
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
                    token,
                    peer,
                    buf: Vec::with_capacity(IN_CAP),
                    pos: 0,
                    out: Vec::with_capacity(OUT_CAP),
                    out_off: 0,
                    pending_sf: None,
                    close_after_write: false,
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
            (
                c.hdr_t0.unwrap_or(c.last_rw),
                router.cfg.header_timeout_ms,
                false,
            )
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
