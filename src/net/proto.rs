//! Shared HTTP/2 and HTTP/3 adapter: `http::Request` → `In` → `Out`.

use std::net::SocketAddr;

use bytes::Bytes;
use http::header::{HeaderName, HeaderValue};

use crate::error::ServeError;
use crate::flags::FlagSet;
use crate::io::{Body, HeaderView, In, Out};
use crate::parse::{looks_like_json, scan_json};
use crate::route::Router;
use crate::status::Status;

pub struct Parts {
    pub method: crate::io::Method,
    pub path: String,
    pub query: String,
    pub headers: Vec<(String, String)>,
    pub body: Bytes,
}

pub fn parts_from_http(req: http::Request<Bytes>) -> Result<Parts, ServeError> {
    let method = crate::io::Method::parse(req.method().as_str()).ok_or(ServeError::Parse)?;
    let path = req.uri().path();
    let path = if path.is_empty() {
        "/".to_string()
    } else {
        path.to_string()
    };
    let query = req.uri().query().unwrap_or("").to_string();
    let mut headers = Vec::with_capacity(req.headers().len());
    for (k, v) in req.headers() {
        let Ok(val) = v.to_str() else { continue };
        headers.push((k.as_str().to_string(), val.to_string()));
    }
    Ok(Parts {
        method,
        path,
        query,
        headers,
        body: req.into_body(),
    })
}

pub async fn dispatch_parts(router: &Router, parts: &Parts, peer: SocketAddr) -> Out {
    if parts.body.len() > router.cfg.max_body_bytes {
        return page(router, 413, "body");
    }
    if looks_like_json(&parts.body) {
        if let Err(e) = scan_json(&parts.body, router.cfg.max_json_depth) {
            return page(router, e.status(), "json");
        }
    }
    let pairs: Vec<(&str, &str)> = parts
        .headers
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let body = if parts.body.is_empty() {
        Body::Empty
    } else if looks_like_json(&parts.body) {
        Body::Json(&parts.body)
    } else {
        Body::Raw(&parts.body)
    };
    let req = In {
        method: parts.method,
        path: &parts.path,
        query: &parts.query,
        headers: HeaderView { pairs },
        body,
        peer,
        flags: FlagSet::empty(),
    };
    if router.has_async() {
        router.dispatch_async(req).await
    } else {
        router.dispatch(req)
    }
}

pub fn out_to_http(out: &Out) -> http::Response<()> {
    let mut b = http::Response::builder().status(out.status.as_u16());
    if let Some(hs) = b.headers_mut() {
        for (k, v) in &out.headers {
            if hop_by_hop(k) {
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
    b.body(()).unwrap_or_else(|_| {
        http::Response::builder()
            .status(500)
            .body(())
            .unwrap_or_else(|_| http::Response::new(()))
    })
}

fn hop_by_hop(k: &str) -> bool {
    k.eq_ignore_ascii_case("connection")
        || k.eq_ignore_ascii_case("keep-alive")
        || k.eq_ignore_ascii_case("proxy-connection")
        || k.eq_ignore_ascii_case("transfer-encoding")
        || k.eq_ignore_ascii_case("upgrade")
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
