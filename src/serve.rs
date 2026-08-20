//! Accept loop. tokio + socket2 listener. Criticality C2.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::align::STATE_ON;
use crate::atom::AtomCtx;
use crate::error::ServeError;
use crate::flags::FlagSet;
use crate::io::{Body, HeaderView, In, Out};
use crate::listen::{self, ListenOpt};
use crate::num::{u16_to_slice, usize_to_slice};
use crate::parse::{find_header_end, looks_like_json, parse_request_owned, scan_json};
use crate::route::Router;

pub struct Running {
    pub local: std::net::SocketAddr,
    pub ctx: Arc<AtomCtx>,
}

pub async fn run(router: Arc<Router>, ctx: Arc<AtomCtx>) -> Result<(), ServeError> {
    let addr: std::net::SocketAddr = router
        .cfg
        .bind
        .parse()
        .map_err(|_| ServeError::Config("bind".into()))?;
    let opt = ListenOpt {
        nodelay: router.cfg.tcp_nodelay,
        reuseport: router.cfg.so_reuseport,
        fastopen: router.cfg.tcp_fastopen,
        backlog: router.cfg.backlog,
    };
    let bound = listen::bind(addr, &opt)?;
    let local = bound.local;
    let listener = TcpListener::from_std(bound.listener)?;
    ctx.signal.v.store(STATE_ON, Ordering::Release);
    tracing::info!(%local, "atomos listen");
    loop {
        if ctx.stop.v.load(Ordering::Acquire) != 0 {
            break;
        }
        let sleep = tokio::time::sleep(Duration::from_millis(200));
        tokio::select! {
            _ = sleep => continue,
            acc = listener.accept() => {
                let (stream, peer) = acc?;
                let router = router.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle(stream, peer, router).await {
                        tracing::debug!(%e, "conn");
                    }
                });
            }
        }
    }
    Ok(())
}

async fn handle(
    mut stream: tokio::net::TcpStream,
    peer: std::net::SocketAddr,
    router: Arc<Router>,
) -> Result<(), ServeError> {
    let _ = stream.set_nodelay(router.cfg.tcp_nodelay);
    let mut buf = Vec::with_capacity(4096);
    let timeout = Duration::from_millis(router.cfg.request_timeout_ms.max(1_000));
    loop {
        let header_end = loop {
            if let Some(n) = find_header_end(&buf) {
                break n;
            }
            if buf.len() > router.cfg.max_header_bytes {
                write_out(&mut stream, &quick_err(400, "headers")).await?;
                return Ok(());
            }
            let mut tmp = [0u8; 2048];
            let n = tokio::time::timeout(timeout, stream.read(&mut tmp))
                .await
                .map_err(|_| ServeError::Timeout)??;
            if n == 0 {
                return Ok(());
            }
            buf.extend_from_slice(&tmp[..n]);
        };
        let (method, path, query, hdrs, n, clen, ka) =
            match parse_request_owned(&buf[..header_end.min(buf.len())], router.cfg.max_header_bytes)
            {
                Ok(v) => v,
                Err(_) => {
                    write_out(&mut stream, &quick_err(400, "parse")).await?;
                    return Ok(());
                }
            };
        let _ = n;
        if clen > router.cfg.max_body_bytes {
            write_out(&mut stream, &quick_err(413, "body")).await?;
            return Ok(());
        }
        while buf.len() < header_end + clen {
            let mut tmp = [0u8; 4096];
            let n = tokio::time::timeout(timeout, stream.read(&mut tmp))
                .await
                .map_err(|_| ServeError::Timeout)??;
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&tmp[..n]);
        }
        if buf.len() < header_end + clen {
            write_out(&mut stream, &quick_err(400, "body")).await?;
            return Ok(());
        }
        let body_bytes = &buf[header_end..header_end + clen];
        if looks_like_json(body_bytes) {
            if let Err(e) = scan_json(body_bytes, router.cfg.max_json_depth) {
                write_out(&mut stream, &quick_err(e.status(), "json")).await?;
                let rest = buf.split_off(header_end + clen);
                buf = rest;
                if !ka {
                    return Ok(());
                }
                continue;
            }
        }
        let arena = bumpalo::Bump::with_capacity(4096);
        let path_a = arena.alloc_str(&path);
        let query_a = arena.alloc_str(&query);
        let mut pairs: Vec<(&str, &str)> = Vec::new();
        for (k, v) in &hdrs {
            pairs.push((&*arena.alloc_str(k), &*arena.alloc_str(v)));
        }
        let body = if clen == 0 {
            Body::Empty
        } else if looks_like_json(body_bytes) {
            Body::Json(arena.alloc_slice_copy(body_bytes))
        } else {
            Body::Raw(arena.alloc_slice_copy(body_bytes))
        };
        let req = In {
            method,
            path: path_a,
            query: query_a,
            headers: HeaderView { pairs },
            body,
            peer,
            flags: FlagSet::empty(),
        };
        let out = router.dispatch_async(req).await;
        write_out(&mut stream, &out).await?;
        drop(arena);
        let rest = buf.split_off(header_end + clen);
        buf = rest;
        if !ka {
            return Ok(());
        }
    }
}

fn quick_err(code: u16, detail: &'static str) -> Out {
    let page = crate::error_page::ErrorPage::builtin();
    let st = crate::status::Status::from_u16(code);
    Out {
        status: st,
        reason: None,
        headers: vec![("Content-Type".into(), "text/html; charset=utf-8".into())],
        body: crate::io::OutBody::Raw(page.render(st, detail)),
        cache: crate::io::CacheDirective::No,
        flags: FlagSet::empty(),
    }
}

async fn write_out(
    stream: &mut tokio::net::TcpStream,
    out: &Out,
) -> Result<(), ServeError> {
    let mut line = [0u8; 64];
    // HTTP/1.1 SSS phrase\r\n
    let mut i = 0;
    let pfx = b"HTTP/1.1 ";
    line[..pfx.len()].copy_from_slice(pfx);
    i += pfx.len();
    i += u16_to_slice(out.status.as_u16(), &mut line[i..]);
    line[i] = b' ';
    i += 1;
    let phrase = out
        .reason
        .as_deref()
        .unwrap_or_else(|| out.status.phrase());
    let pb = phrase.as_bytes();
    line[i..i + pb.len()].copy_from_slice(pb);
    i += pb.len();
    line[i..i + 2].copy_from_slice(b"\r\n");
    i += 2;
    stream.write_all(&line[..i]).await?;
    let body = out.body.as_bytes();
    // Content-Length
    stream.write_all(b"Content-Length: ").await?;
    let mut nb = [0u8; 24];
    let n = usize_to_slice(body.len(), &mut nb);
    stream.write_all(&nb[..n]).await?;
    stream.write_all(b"\r\n").await?;
    stream.write_all(b"Connection: keep-alive\r\n").await?;
    stream
        .write_all(b"X-Content-Type-Options: nosniff\r\n")
        .await?;
    stream.write_all(b"X-Frame-Options: DENY\r\n").await?;
    stream.write_all(b"Referrer-Policy: no-referrer\r\n").await?;
    for (k, v) in &out.headers {
        stream.write_all(k.as_bytes()).await?;
        stream.write_all(b": ").await?;
        stream.write_all(v.as_bytes()).await?;
        stream.write_all(b"\r\n").await?;
    }
    stream.write_all(b"\r\n").await?;
    if out.method_head_skip() {
        // not tracked; HEAD emptied body already
    }
    if !body.is_empty() {
        stream.write_all(body).await?;
    }
    let _ = Bytes::new();
    Ok(())
}

impl Out {
    fn method_head_skip(&self) -> bool {
        false
    }
}
