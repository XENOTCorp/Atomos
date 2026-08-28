//! Pinned thread-per-core accept. HTTP/1.1 + h2c + TLS h2 on TCP; h3 on UDP.
//! Criticality C2.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use tokio::net::TcpListener;

use crate::align::STATE_ON;
use crate::atom::AtomCtx;
use crate::error::ServeError;
use crate::listen::{self, ListenOpt};
use crate::route::Router;
use crate::tls::TlsHold;

mod accept;
mod detect;
mod h1;
use accept::accept_loop;

pub(crate) const H2_PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

pub struct Running {
    pub local: std::net::SocketAddr,
    pub ctx: Arc<AtomCtx>,
}

struct StopOnDrop(Arc<AtomCtx>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.stop.v.store(1, Ordering::Release);
    }
}

pub async fn run(router: Arc<Router>, ctx: Arc<AtomCtx>) -> Result<(), ServeError> {
    let _guard = StopOnDrop(ctx.clone());
    let mut addr: std::net::SocketAddr = router
        .cfg
        .bind
        .parse()
        .map_err(|_| ServeError::Config("bind".into()))?;
    if router.cfg.refuse_ports.contains(&addr.port()) {
        return Err(ServeError::Config(
            format!("bind port {} is in refuse_ports", addr.port()).into(),
        ));
    }
    let opt = ListenOpt {
        nodelay: router.cfg.tcp_nodelay,
        reuseport: router.cfg.so_reuseport,
        fastopen: router.cfg.tcp_fastopen,
        backlog: router.cfg.backlog,
    };
    let n = if opt.reuseport {
        router.cfg.workers.max(1)
    } else {
        1
    };
    let tls = Arc::new(TlsHold::with_opts(
        router.cfg.tls_cert.clone(),
        router.cfg.tls_key.clone(),
        router.cfg.tls_ocsp.clone(),
        router.cfg.tls_ticket_lifetime_secs,
    ));
    if router.cfg.http3 {
        let _ = tls.get()?;
    }

    let mut tcps = Vec::with_capacity(n as usize);
    for i in 0..n {
        let bound = listen::bind(addr, &opt)?;
        if i == 0 {
            addr = bound.local;
        }
        tcps.push(bound);
    }
    let mut udps = Vec::new();
    if router.cfg.http3 {
        for _ in 0..n {
            match listen::bind_udp(addr, opt.reuseport) {
                Ok(s) => udps.push(s),
                Err(e) => {
                    tracing::warn!(%e, "http3 udp bind");
                    break;
                }
            }
        }
    }
    crate::ops::jail::after_bind(&router.cfg)?;

    ctx.signal.v.store(STATE_ON, Ordering::Release);
    tracing::info!(
        local = %addr,
        workers = n,
        pin = router.cfg.cpu_pin,
        http2 = router.cfg.http2,
        http3 = router.cfg.http3 && !udps.is_empty(),
        "atomos listen"
    );

    let mut dones = Vec::with_capacity(n as usize);
    for i in 0..n {
        let tcp = tcps.remove(0);
        let udp = if udps.is_empty() {
            None
        } else {
            Some(udps.remove(0))
        };
        let router = router.clone();
        let ctx = ctx.clone();
        let tls = tls.clone();
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        dones.push(done_rx);
        std::thread::Builder::new()
            .name(format!("atomos-{i}"))
            .spawn(move || {
                if router.cfg.cpu_pin {
                    let _ = crate::pin_cpu::pin_to_cpu(i as usize);
                }
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .max_blocking_threads(2)
                    .build()
                {
                    Ok(rt) => rt,
                    Err(_) => {
                        let _ = done_tx.send(());
                        return;
                    }
                };
                rt.block_on(worker(tcp.listener, udp, router, ctx, tls));
                let _ = done_tx.send(());
            })
            .map_err(ServeError::Io)?;
    }

    for d in dones {
        let _ = d.await;
    }
    Ok(())
}

async fn worker(
    listener: std::net::TcpListener,
    udp: Option<std::net::UdpSocket>,
    router: Arc<Router>,
    ctx: Arc<AtomCtx>,
    tls: Arc<TlsHold>,
) {
    let listener = match TcpListener::from_std(listener) {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!(%e, "tcp from_std");
            return;
        }
    };
    if let Some(udp) = udp {
        match tls.get() {
            Ok(set) => match crate::h3serve::endpoint_from_std(udp, &set) {
                Ok(ep) => {
                    let router = router.clone();
                    let ctx = ctx.clone();
                    tokio::spawn(crate::h3serve::accept_loop(ep, router, ctx));
                }
                Err(e) => tracing::warn!(%e, "http3 endpoint"),
            },
            Err(e) => tracing::warn!(%e, "http3 tls"),
        }
    }
    accept_loop(listener, router, ctx, tls).await;
}

