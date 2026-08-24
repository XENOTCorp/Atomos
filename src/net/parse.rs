//! httparse + JSON first-byte/depth scan. Zero-copy path/query.
//! Domain: a receive buffer. Bound: header and body caps from config.

use crate::error::ServeError;
use crate::io::Method;

pub type OwnedReq = (
    Method,
    String,
    String,
    Vec<(String, String)>,
    usize,
    usize,
    bool,
);

/// Views into `buf`. `headers` names/values and path/query borrow `buf`.
pub struct Parsed<'a> {
    pub method: Method,
    pub path: &'a str,
    pub query: &'a str,
    pub headers: Vec<(&'a str, &'a str)>,
    pub header_end: usize,
    pub content_length: usize,
    pub keepalive: bool,
}

pub enum ParseStatus<'a> {
    Complete(Parsed<'a>),
    Partial,
}

pub fn parse_request(buf: &[u8], max_header: usize) -> Result<ParseStatus<'_>, ServeError> {
    if buf.len() > max_header
        && find_header_end(buf).map(|n| n > max_header).unwrap_or(true)
    {
        return Err(ServeError::Parse);
    }
    let mut headers = [httparse::EMPTY_HEADER; 64];
    let mut req = httparse::Request::new(&mut headers);
    let n = match req.parse(buf) {
        Ok(httparse::Status::Complete(n)) => n,
        Ok(httparse::Status::Partial) => return Ok(ParseStatus::Partial),
        Err(_) => return Err(ServeError::Parse),
    };
    let method = Method::parse(req.method.ok_or(ServeError::Parse)?).ok_or(ServeError::Parse)?;
    let target = req.path.ok_or(ServeError::Parse)?;
    let (path, query) = split_target(target);
    let mut content_length = 0usize;
    let mut saw_cl = false;
    let mut saw_te = false;
    let mut keepalive = req.version.unwrap_or(1) >= 1;
    let mut pairs = Vec::with_capacity(req.headers.len());
    for h in req.headers.iter() {
        let val = std::str::from_utf8(h.value).map_err(|_| ServeError::Parse)?;
        if h.name.eq_ignore_ascii_case("content-length") {
            content_length = val.parse().map_err(|_| ServeError::Parse)?;
            saw_cl = true;
        }
        if h.name.eq_ignore_ascii_case("transfer-encoding") {
            saw_te = true;
        }
        if h.name.eq_ignore_ascii_case("connection") && val.eq_ignore_ascii_case("close") {
            keepalive = false;
        }
        if h.name.eq_ignore_ascii_case("connection") && val.eq_ignore_ascii_case("keep-alive") {
            keepalive = true;
        }
        pairs.push((h.name, val));
    }
    if saw_cl && saw_te {
        return Err(ServeError::Parse);
    }
    Ok(ParseStatus::Complete(Parsed {
        method,
        path,
        query,
        headers: pairs,
        header_end: n,
        content_length,
        keepalive,
    }))
}

pub fn parse_request_owned(buf: &[u8], max_header: usize) -> Result<OwnedReq, ServeError> {
    match parse_request(buf, max_header)? {
        ParseStatus::Partial => Err(ServeError::Parse),
        ParseStatus::Complete(p) => Ok((
            p.method,
            p.path.to_string(),
            p.query.to_string(),
            p.headers
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            p.header_end,
            p.content_length,
            p.keepalive,
        )),
    }
}

pub fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4)
}

fn split_target(target: &str) -> (&str, &str) {
    match target.split_once('?') {
        Some((p, q)) => (p, q),
        None => (target, ""),
    }
}

/// Depth-limited JSON scan. O(n), O(1) extra. Rejects depth > max.
pub fn scan_json(body: &[u8], max_depth: u32) -> Result<(), ServeError> {
    let mut i = 0usize;
    while i < body.len() && body[i].is_ascii_whitespace() {
        i += 1;
    }
    if i >= body.len() {
        return Err(ServeError::JsonInvalid);
    }
    let first = body[i];
    if first != b'{' && first != b'[' {
        return Err(ServeError::JsonInvalid);
    }
    let mut depth: u32 = 0;
    let mut in_str = false;
    let mut esc = false;
    while i < body.len() {
        let b = body[i];
        if in_str {
            if esc {
                esc = false;
            } else if b == b'\\' {
                esc = true;
            } else if b == b'"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'{' | b'[' => {
                depth = depth
                    .checked_add(1)
                    .ok_or(ServeError::JsonDepth)?;
                if depth > max_depth {
                    return Err(ServeError::JsonDepth);
                }
            }
            b'}' | b']' => {
                if depth == 0 {
                    return Err(ServeError::JsonInvalid);
                }
                depth -= 1;
            }
            _ => {}
        }
        i += 1;
    }
    if in_str || depth != 0 {
        return Err(ServeError::JsonInvalid);
    }
    Ok(())
}

pub fn looks_like_json(body: &[u8]) -> bool {
    let mut i = 0;
    while i < body.len() && body[i].is_ascii_whitespace() {
        i += 1;
    }
    matches!(body.get(i), Some(b'{' | b'['))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_deep_array() {
        let s = vec![b'['; 40];
        assert!(matches!(scan_json(&s, 32), Err(ServeError::JsonDepth)));
    }

    #[test]
    fn accepts_object_depth_2() {
        assert!(scan_json(br#"{"a":{"b":1}}"#, 32).is_ok());
    }

    #[test]
    fn parses_get_path() {
        let raw = b"GET /ai.txt HTTP/1.1\r\nHost: x\r\n\r\n";
        let (m, path, _, _, _, _, _) = parse_request_owned(raw, 16384).unwrap();
        assert_eq!(path, "/ai.txt");
        assert_eq!(m, Method::Get);
    }

    #[test]
    fn borrowed_parse_points_into_buf() {
        let raw = b"GET /api/health HTTP/1.1\r\nHost: x\r\n\r\n";
        match parse_request(raw, 16384).unwrap() {
            ParseStatus::Complete(p) => {
                assert_eq!(p.path, "/api/health");
                assert_eq!(p.method, Method::Get);
                assert!(p.keepalive);
                assert_eq!(p.content_length, 0);
            }
            ParseStatus::Partial => panic!("expected complete"),
        }
    }

    #[test]
    fn cl_plus_te_is_error() {
        let raw = b"POST / HTTP/1.1\r\nHost: x\r\nContent-Length: 4\r\nTransfer-Encoding: chunked\r\n\r\nWAIT";
        assert!(matches!(parse_request(raw, 16384), Err(ServeError::Parse)));
    }

    #[test]
    fn partial_headers_are_partial() {
        let raw = b"GET / HTTP/1.1\r\nHost: x\r\n";
        match parse_request(raw, 16384).unwrap() {
            ParseStatus::Partial => {}
            ParseStatus::Complete(_) => panic!("expected partial"),
        }
    }
}
