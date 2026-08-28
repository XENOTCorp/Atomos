//! Tokio HTTP/1.1 handler.
use std::cell::RefCell;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use crate::encode::encode_response;
use crate::error::ServeError;
use crate::error_page::ErrorPage;
use crate::flags::FlagSet;
use crate::io::{Body, HeaderView, In, Out};
use crate::parse::{looks_like_json, parse_request, scan_json, ParseStatus};
use crate::route::Router;
use crate::status::Status;

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

pub(crate) async fn read_more<S>(
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

pub(crate) async fn write_out<S>(stream: &mut S, out: &Out) -> Result<(), ServeError>
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
