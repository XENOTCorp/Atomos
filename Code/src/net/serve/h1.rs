//! Tokio HTTP/1.1 handler.
use crate::cache::CachedResponse;
use crate::encode::{encode_head, encode_response};
use crate::error::ServeError;
use crate::error_page::ErrorPage;
use crate::flags::FlagSet;
use crate::io::{Body, HeaderView, In, Out};
use crate::parse::{
    decode_chunked_into, looks_like_json, parse_request_with_limits, scan_json, ParseStatus,
};
use crate::route::Router;
use crate::status::Status;
use std::cell::RefCell;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

thread_local! {
    static ENC: RefCell<Vec<u8>> = RefCell::new(Vec::with_capacity(2048));
}

pub(crate) async fn handle_h1<S>(
    mut stream: S,
    peer: std::net::SocketAddr,
    router: Arc<Router>,
) -> Result<(), ServeError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut buf = Vec::with_capacity(4096);
    let timeout = Duration::from_millis(router.cfg.request_timeout_ms.max(1));
    let mut tmp = [0u8; 4096];
    loop {
        let (out, used, ka, head) = loop {
            match parse_request_with_limits(
                &buf,
                router.cfg.max_header_bytes,
                router.cfg.max_body_bytes,
            ) {
                Ok(ParseStatus::Partial) => {
                    if buf.len()
                        > router
                            .cfg
                            .max_header_bytes
                            .saturating_add(router.cfg.max_body_bytes)
                    {
                        write_out(&mut stream, &quick_err(413, "body"), false).await?;
                        return Ok(());
                    }
                    let n = read_more(&mut stream, &mut tmp, timeout).await?;
                    if n == 0 {
                        return Ok(());
                    }
                    buf.extend_from_slice(&tmp[..n]);
                }
                Ok(ParseStatus::Complete(p)) => {
                    if p.content_length > router.cfg.max_body_bytes {
                        write_out(&mut stream, &quick_err(413, "body"), false).await?;
                        return Ok(());
                    }
                    let need = p.wire_end;
                    if buf.len() < need {
                        let n = read_more(&mut stream, &mut tmp, timeout).await?;
                        if n == 0 {
                            write_out(&mut stream, &quick_err(400, "body"), false).await?;
                            return Ok(());
                        }
                        buf.extend_from_slice(&tmp[..n]);
                        continue;
                    }
                    if p.upgrade {
                        write_out(&mut stream, &quick_err(426, "upgrade"), false).await?;
                        return Ok(());
                    }
                    let mut decoded = Vec::new();
                    let body_bytes: &[u8] = if p.chunked {
                        decode_chunked_into(&buf[p.header_end..need], &mut decoded)?;
                        &decoded
                    } else {
                        &buf[p.header_end..need]
                    };
                    if looks_like_json(body_bytes) {
                        if let Err(e) = scan_json(body_bytes, router.cfg.max_json_depth) {
                            write_out(&mut stream, &quick_err(e.status(), "json"), false).await?;
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
                    let head = p.method == crate::io::Method::Head;
                    if let Some(cached) = router.cached_h1(&req) {
                        drop(req);
                        match cached {
                            CachedResponse::Wire { bytes, .. } => stream.write_all(&bytes).await?,
                            CachedResponse::Response(out) => {
                                write_out(&mut stream, &out, head).await?
                            }
                        }
                        compact(&mut buf, need);
                        if !ka {
                            return Ok(());
                        }
                        continue;
                    }
                    let t0 = std::time::Instant::now();
                    let mut out = tokio::time::timeout(
                        Duration::from_millis(router.cfg.module_timeout_ms.max(1)),
                        router.dispatch_async(req),
                    )
                    .await
                    .unwrap_or_else(|_| Out::empty(Status::GATEWAY_TIMEOUT));
                    if t0.elapsed().as_millis() as u64 > router.cfg.module_timeout_ms.max(1) {
                        out = crate::io::Out::empty(crate::status::Status::GATEWAY_TIMEOUT);
                    }
                    break (out, need, ka, head);
                }
                Err(error) => {
                    write_out(&mut stream, &quick_err(error.status(), "parse"), false).await?;
                    return Ok(());
                }
            }
        };
        write_out(&mut stream, &out, head).await?;
        compact(&mut buf, used);
        if !ka {
            return Ok(());
        }
    }
}

pub(crate) async fn read_more<S>(
    stream: &mut S,
    tmp: &mut [u8],
    timeout: Duration,
) -> Result<usize, ServeError>
where
    S: AsyncRead + Unpin,
{
    tokio::time::timeout(timeout, stream.read(tmp))
        .await
        .map_err(|_| ServeError::Timeout)?
        .map_err(ServeError::from)
}

pub(crate) fn compact(buf: &mut Vec<u8>, used: usize) {
    let n = buf.len();
    if used >= n {
        buf.clear();
        return;
    }
    buf.copy_within(used.., 0);
    buf.truncate(n - used);
}

pub(crate) fn quick_err(code: u16, detail: &'static str) -> Out {
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

pub(crate) async fn write_out<S>(stream: &mut S, out: &Out, head: bool) -> Result<(), ServeError>
where
    S: AsyncWrite + Unpin,
{
    let mut buf = ENC.with(|cell| cell.replace(Vec::new()));
    if buf.capacity() < 512 {
        buf = Vec::with_capacity(2048);
    }
    let r = async {
        if let crate::io::OutBody::File(file) = &out.body {
            encode_head(out, &mut buf);
            stream.write_all(&buf).await?;
            if !head && out.status.allows_body() {
                let mut chunks = crate::net::file_body::FileChunks::new(file);
                while let Some(bytes) = chunks.next().await? {
                    stream.write_all(&bytes).await?;
                }
            }
        } else {
            if head {
                encode_head(out, &mut buf);
            } else {
                encode_response(out, &mut buf);
            }
            stream.write_all(&buf).await?;
        }
        Ok::<_, ServeError>(())
    }
    .await;
    ENC.with(|cell| {
        let _ = cell.replace(buf);
    });
    r
}
