//! Pinned thread-per-core accept. HTTP/1.1 + h2c + TLS h2 on TCP; h3 on UDP.
//! Criticality C2.

use std::cell::RefCell;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

use crate::align::STATE_ON;
use crate::atom::AtomCtx;
use crate::encode::encode_response;
use crate::error::ServeError;
use crate::error_page::ErrorPage;
use crate::flags::FlagSet;
use crate::io::{Body, HeaderView, In, Out};
use crate::listen::{self, ListenOpt};
use crate::parse::{looks_like_json, parse_request, scan_json, ParseStatus};
use crate::route::Router;
use crate::status::Status;
use crate::tls::TlsHold;

const H2_PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

thread_local! {
    static ENC: RefCell<Vec<u8>> = RefCell::new(Vec::with_capacity(2048));
}

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

async fn accept_loop(
    listener: TcpListener,
    router: Arc<Router>,
    ctx: Arc<AtomCtx>,
    tls: Arc<TlsHold>,
) {
    loop {
        if ctx.stop.v.load(Ordering::Acquire) != 0 {
            break;
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(200)) => {
                if ctx.stop.v.load(Ordering::Acquire) != 0 {
                    break;
                }
            }
            acc = listener.accept() => {
                let Ok((stream, peer)) = acc else { continue };
                let router = router.clone();
                let tls = tls.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_tcp(stream, peer, router, tls).await {
                        tracing::debug!(%e, "conn");
                    }
                });
            }
        }
    }
}

async fn handle_tcp(
    stream: tokio::net::TcpStream,
    peer: std::net::SocketAddr,
    router: Arc<Router>,
    tls: Arc<TlsHold>,
) -> Result<(), ServeError> {
    let _ = stream.set_nodelay(router.cfg.tcp_nodelay);
    let mut peek = [0u8; 24];
    let n = stream.peek(&mut peek).await?;
    if n == 0 {
        return Ok(());
    }
    if peek[0] == 0x16 && (router.cfg.http2 || router.cfg.http3) {
        let set = tls.get()?;
        let acceptor = TlsAcceptor::from(set.tcp.clone());
        let tls_stream = acceptor
            .accept(stream)
            .await
            .map_err(|e| ServeError::Io(std::io::Error::other(e)))?;
        let alpn = tls_stream.get_ref().1.alpn_protocol().map(|p| p.to_vec());
        if router.cfg.http2 && alpn.as_deref() == Some(b"h2") {
            return crate::h2serve::handle(tls_stream, peer, router).await;
        }
        return handle_h1(tls_stream, peer, router).await;
    }
    if router.cfg.http2 && (n >= H2_PREFACE.len() && peek.starts_with(H2_PREFACE)
        || n >= 3 && peek.starts_with(b"PRI"))
    {
        return crate::h2serve::handle(stream, peer, router).await;
    }
    handle_h1(stream, peer, router).await
}

async fn handle_h1<S>(
    mut stream: S,
    peer: std::net::SocketAddr,
    router: Arc<Router>,
) -> Result<(), ServeError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut buf = Vec::with_capacity(4096);
    let timeout = Duration::from_millis(router.cfg.request_timeout_ms.max(1_000));
    let mut tmp = [0u8; 4096];
    loop {
        let (out, used, ka) = loop {
            match parse_request(&buf, router.cfg.max_header_bytes) {
                Ok(ParseStatus::Partial) => {
                    if buf.len() > router.cfg.max_header_bytes {
                        write_out(&mut stream, &quick_err(400, "headers")).await?;
                        return Ok(());
                    }
                    let n = read_more(&mut stream, &mut tmp, timeout, buf.is_empty()).await?;
                    if n == 0 {
                        return Ok(());
                    }
                    buf.extend_from_slice(&tmp[..n]);
                }
                Ok(ParseStatus::Complete(p)) => {
                    if p.content_length > router.cfg.max_body_bytes {
                        write_out(&mut stream, &quick_err(413, "body")).await?;
                        return Ok(());
                    }
                    let need = p.header_end + p.content_length;
                    if buf.len() < need {
                        let n = read_more(&mut stream, &mut tmp, timeout, false).await?;
                        if n == 0 {
                            write_out(&mut stream, &quick_err(400, "body")).await?;
                            return Ok(());
                        }
                        buf.extend_from_slice(&tmp[..n]);
                        continue;
                    }
                    if p.content_length == 0 {
                        if let Some(wire) = router.cache.get_wire(p.method, p.path, p.query) {
                            let ka = p.keepalive;
                            drop(p);
                            stream.write_all(wire.as_ref()).await?;
                            compact(&mut buf, need);
                            if !ka {
                                return Ok(());
                            }
                            continue;
                        }
                    }
                    let body_bytes = &buf[p.header_end..need];
                    if looks_like_json(body_bytes) {
                        if let Err(e) = scan_json(body_bytes, router.cfg.max_json_depth) {
                            write_out(&mut stream, &quick_err(e.status(), "json")).await?;
                            let ka = p.keepalive;
                            compact(&mut buf, need);
                            if !ka {
                                return Ok(());
                            }
                            continue;
                        }
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
                        peer,
                        flags: FlagSet::empty(),
                    };
                    let ka = p.keepalive;
                    let out = if router.has_async() {
                        router.dispatch_async(req).await
                    } else {
                        router.dispatch(req)
                    };
                    break (out, need, ka);
                }
                Err(_) => {
                    write_out(&mut stream, &quick_err(400, "parse")).await?;
                    return Ok(());
                }
            }
        };
        write_out(&mut stream, &out).await?;
        compact(&mut buf, used);
        if !ka {
            return Ok(());
        }
    }
}

async fn read_more<S>(
    stream: &mut S,
    tmp: &mut [u8],
    timeout: Duration,
    idle: bool,
) -> Result<usize, ServeError>
where
    S: AsyncRead + Unpin,
{
    if idle {
        tokio::time::timeout(timeout, stream.read(tmp))
            .await
            .map_err(|_| ServeError::Timeout)?
            .map_err(ServeError::from)
    } else {
        stream.read(tmp).await.map_err(ServeError::from)
    }
}

fn compact(buf: &mut Vec<u8>, used: usize) {
    let n = buf.len();
    if used >= n {
        buf.clear();
        return;
    }
    buf.copy_within(used.., 0);
    buf.truncate(n - used);
}

fn quick_err(code: u16, detail: &'static str) -> Out {
    static PAGE: std::sync::OnceLock<ErrorPage> = std::sync::OnceLock::new();
    let page = PAGE.get_or_init(ErrorPage::builtin);
    let st = Status::from_u16(code);
    Out {
        status: st,
        reason: None,
        headers: vec![("Content-Type".into(), "text/html; charset=utf-8".into())],
        body: crate::io::OutBody::Raw(page.render(st, detail)),
        cache: crate::io::CacheDirective::No,
        flags: FlagSet::empty(),
    }
}

async fn write_out<S>(stream: &mut S, out: &Out) -> Result<(), ServeError>
where
    S: AsyncWrite + Unpin,
{
    let mut buf = ENC.with(|cell| cell.replace(Vec::new()));
    if buf.capacity() < 512 {
        buf = Vec::with_capacity(2048);
    }
    if matches!(out.body, crate::io::OutBody::File(_)) {
        // Tokio H1 cannot sendfile (generic AsyncWrite, possibly TLS):
        // materialize the file on a blocking thread, then encode bytes.
        let mut o = out.clone();
        o.body = crate::io::OutBody::Raw(crate::proto::materialize_file_body(&o).await);
        encode_response(&o, &mut buf);
    } else {
        encode_response(out, &mut buf);
    }
    let r = stream.write_all(&buf).await;
    ENC.with(|cell| {
        let _ = cell.replace(buf);
    });
    r?;
    Ok(())
}


