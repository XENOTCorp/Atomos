//! Shared HTTP/2 and HTTP/3 adapter: `http::Request` → `In` → `Out`.

use std::net::SocketAddr;

use bytes::Bytes;
use http::header::{HeaderName, HeaderValue};

use crate::error::ServeError;
use crate::flags::FlagSet;
use crate::io::{Body, HeaderView, In, Out, OutBody};
use crate::parse::{looks_like_json, scan_json};
use crate::route::Router;
use crate::status::Status;

/// Uncompressed header-size estimate at the application boundary, shared by
/// both protocols. This is not an exact HPACK/QPACK encoded size.
pub fn raw_header_bytes(head: &http::request::Parts) -> u64 {
    let path = head
        .uri
        .path_and_query()
        .map_or(1, |path| path.as_str().len());
    let mut bytes = head.method.as_str().len() + path;
    for (name, value) in &head.headers {
        bytes += name.as_str().len() + value.as_bytes().len() + 4;
    }
    bytes as u64
}

/// Request head, borrowed from the `http::Request`: no per-header
/// `String` copies on the tokio paths (the H1 path's zero-alloc
/// discipline applied to H2/H3 dispatch).
pub struct Parts<'a> {
    pub method: crate::io::Method,
    pub path: &'a str,
    pub query: &'a str,
    pub headers: Vec<(&'a str, &'a str)>,
    pub body: Bytes,
}

pub fn parts_from_http<'a>(req: &'a http::Request<Bytes>) -> Result<Parts<'a>, ServeError> {
    let method = crate::io::Method::parse(req.method().as_str()).ok_or(ServeError::Parse)?;
    let path = req.uri().path();
    let path = if path.is_empty() { "/" } else { path };
    let query = req.uri().query().unwrap_or("");
    let mut headers = Vec::with_capacity(req.headers().len());
    for (k, v) in req.headers() {
        // h2/h3 HeaderValues are validated on construction; a non-UTF8
        // value is skipped exactly as the old String-copy path did.
        let Ok(val) = v.to_str() else { continue };
        headers.push((k.as_str(), val));
    }
    Ok(Parts {
        method,
        path,
        query,
        headers,
        // Bytes clone: refcount bump, not a copy.
        body: req.body().clone(),
    })
}

pub async fn dispatch_parts(router: &Router, parts: Parts<'_>, peer: SocketAddr) -> Out {
    if parts.body.len() > router.cfg.max_body_bytes {
        return page(router, 413, "body");
    }
    if looks_like_json(&parts.body) {
        if let Err(e) = scan_json(&parts.body, router.cfg.max_json_depth) {
            return page(router, e.status(), "json");
        }
    }
    let Parts {
        method,
        path,
        query,
        headers,
        body,
    } = parts;
    let body = if body.is_empty() {
        Body::Empty
    } else if looks_like_json(&body) {
        Body::Json(&body)
    } else {
        Body::Raw(&body)
    };
    let req = In {
        method,
        path,
        query,
        headers: HeaderView { pairs: headers },
        body,
        peer,
        flags: FlagSet::empty(),
    };
    router.dispatch_async(req).await
}

/// Streaming dispatch for the tokio paths (h2/h3). The request head is
/// dispatched **while the body is still arriving**: chunks flow to the
/// module through `body_rx` as the transport reads them. Modules that
/// opt into `AsyncStreamModule` consume chunks incrementally; anything
/// else falls back to the buffered `dispatch_parts` (which re-admits
/// and re-validates exactly as before).
pub async fn stream_dispatch(
    router: &Router,
    head: http::request::Parts,
    peer: SocketAddr,
    body_rx: tokio::sync::mpsc::Receiver<Bytes>,
) -> Out {
    if router
        .stream_handler(&head.method, head.uri.path())
        .is_some()
    {
        let req = http::Request::from_parts(head, ());
        router.dispatch_streaming(&req, peer, body_rx).await
    } else {
        // Buffered fallback: collect the channel, then the normal path.
        let mut body = bytes::BytesMut::new();
        let mut rx = body_rx;
        while let Some(c) = rx.recv().await {
            if c.len() > router.cfg.max_body_bytes.saturating_sub(body.len()) {
                return page(router, 413, "body");
            }
            body.extend_from_slice(&c);
        }
        let req = http::Request::from_parts(head, body.freeze());
        let Ok(parts) = parts_from_http(&req) else {
            return page(router, 400, "parse");
        };
        dispatch_parts(router, parts, peer).await
    }
}

pub fn out_to_http(out: &Out) -> http::Response<()> {
    let mut b = http::Response::builder().status(out.status.as_u16());
    if let Some(hs) = b.headers_mut() {
        for (k, v) in &out.headers {
            if crate::net::headers::transport_owned(k) {
                continue;
            }
            let Ok(name) = HeaderName::from_bytes(k.as_bytes()) else {
                continue;
            };
            let Ok(val) = HeaderValue::from_str(v) else {
                continue;
            };
            hs.append(name, val);
        }
    }
    if !matches!(out.body, OutBody::Stream(_)) && out.status.allows_body() {
        b = b.header(http::header::CONTENT_LENGTH, out.body.len());
    }
    b.body(()).unwrap_or_else(|_| {
        http::Response::builder()
            .status(500)
            .body(())
            .unwrap_or_else(|_| http::Response::new(()))
    })
}

fn page(router: &Router, code: u16, detail: &str) -> Out {
    let st = Status::from_u16(code);
    Out {
        status: st,
        reason: None,
        headers: vec![("Content-Type".into(), "text/html; charset=utf-8".into())],
        body: crate::io::OutBody::Raw(router.errors.render(st, detail)),
        cache: crate::io::CacheDirective::No,
        flags: FlagSet::empty(),
    }
}
