//! httparse + JSON first-byte/depth scan. Zero-copy path/query.
//! Domain: a receive buffer. Bound: header and body caps from config.
//!
//! Framing is one function: [`parse_request`]. It is the only place that
//! decides Content-Length, Transfer-Encoding, request-target form, and
//! Host vs absolute-form. Callers must not re-parse those fields.

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
    /// End of this HTTP/1.1 message on the wire, relative to `buf[0]`.
    pub wire_end: usize,
    /// Decoded body length. For identity this equals the Content-Length.
    pub content_length: usize,
    pub chunked: bool,
    pub keepalive: bool,
    /// `Upgrade` is present. The engine must answer 426 and close.
    pub upgrade: bool,
}

pub enum ParseStatus<'a> {
    Complete(Parsed<'a>),
    Partial,
}

/// Reject obs-fold, TAB in a header name, a line longer than `max_header`.
fn scan_header_block(buf: &[u8], max_header: usize) -> Result<Option<usize>, ServeError> {
    let Some(end) = find_header_end(buf) else {
        if buf.len() > max_header {
            return Err(ServeError::Parse);
        }
        return Ok(None);
    };
    if end > max_header {
        return Err(ServeError::Parse);
    }
    let block = &buf[..end];
    let mut i = 0usize;
    let mut first = true;
    while i < block.len() {
        if block[i..].starts_with(b"\r\n") {
            break;
        }
        let Some(rel) = block[i..].windows(2).position(|w| w == b"\r\n") else {
            return Err(ServeError::Parse);
        };
        let line = &block[i..i + rel];
        if line.len() > max_header {
            return Err(ServeError::Parse);
        }
        if !first && (line.starts_with(b" ") || line.starts_with(b"\t")) {
            // obs-fold
            return Err(ServeError::Parse);
        }
        if !first {
            // name TAB or space before colon
            let colon = line.iter().position(|&b| b == b':').ok_or(ServeError::Parse)?;
            let name = &line[..colon];
            if name.is_empty()
                || name.iter().any(|&b| b == b'\t' || b == b' ' || b == 0 || b > 127)
            {
                return Err(ServeError::Parse);
            }
        }
        first = false;
        i += rel + 2;
    }
    Ok(Some(end))
}

fn split_path_query(target: &str) -> (&str, &str) {
    match target.split_once('?') {
        Some((p, q)) => (p, q),
        None => (target, ""),
    }
}

/// Origin-form, asterisk-form, absolute-form. Authority-form only for CONNECT.
/// Returns (path, query, uri-host).
pub fn normalize_target(
    method: Method,
    target: &str,
) -> Result<(&str, &str, Option<&str>), ServeError> {
    if target.is_empty() {
        return Err(ServeError::Parse);
    }
    if target == "*" {
        return Ok(("*", "", None));
    }
    if target.starts_with('/') {
        let (p, q) = split_path_query(target);
        return Ok((p, q, None));
    }
    let rest = if let Some(r) = target.strip_prefix("http://") {
        r
    } else if let Some(r) = target.strip_prefix("https://") {
        r
    } else if method == Method::Connect {
        return Ok((target, "", None));
    } else {
        return Err(ServeError::Parse);
    };
    let slash = rest.find('/').unwrap_or(rest.len());
    let hostport = &rest[..slash];
    if hostport.is_empty() {
        return Err(ServeError::Parse);
    }
    let host = host_without_port(hostport);
    if host.is_empty() {
        return Err(ServeError::Parse);
    }
    let pq = if slash < rest.len() {
        &rest[slash..]
    } else {
        "/"
    };
    let (p, q) = split_path_query(pq);
    Ok((p, q, Some(host)))
}

pub fn host_without_port(hostport: &str) -> &str {
    if let Some(rest) = hostport.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(rest);
    }
    hostport.split(':').next().unwrap_or(hostport)
}

fn te_is_chunked_only(val: &str) -> bool {
    let mut n = 0u8;
    for part in val.split(',') {
        let p = part.trim();
        if p.is_empty() {
            continue;
        }
        n = n.saturating_add(1);
        if !p.eq_ignore_ascii_case("chunked") {
            return false;
        }
    }
    n == 1
}

fn connection_has(val: &str, token: &str) -> bool {
    val.split(',').any(|p| p.trim().eq_ignore_ascii_case(token))
}

/// Walk a chunked body. No chunk extensions. No trailers.
/// `Ok(None)` = need more bytes. `Ok(Some((decoded, wire)))` = complete.
pub fn measure_chunked(src: &[u8]) -> Result<Option<(usize, usize)>, ServeError> {
    let mut i = 0usize;
    let mut decoded = 0usize;
    loop {
        let Some(nl) = src[i..].windows(2).position(|w| w == b"\r\n") else {
            return Ok(None);
        };
        let line = &src[i..i + nl];
        if line.contains(&b';') {
            return Err(ServeError::Parse);
        }
        if line.is_empty() {
            return Err(ServeError::Parse);
        }
        let size = usize::from_str_radix(std::str::from_utf8(line).map_err(|_| ServeError::Parse)?, 16)
            .map_err(|_| ServeError::Parse)?;
        i += nl + 2;
        if size == 0 {
            if src.len() < i + 2 {
                return Ok(None);
            }
            if &src[i..i + 2] != b"\r\n" {
                // trailers refused
                return Err(ServeError::Parse);
            }
            return Ok(Some((decoded, i + 2)));
        }
        if src.len() < i + size + 2 {
            return Ok(None);
        }
        if &src[i + size..i + size + 2] != b"\r\n" {
            return Err(ServeError::Parse);
        }
        decoded = decoded.saturating_add(size);
        i += size + 2;
    }
}

/// Copy decoded chunk bytes into `dst`. `src` is the wire body (after headers).
pub fn decode_chunked_into(src: &[u8], dst: &mut Vec<u8>) -> Result<usize, ServeError> {
    let mut i = 0usize;
    loop {
        let Some(nl) = src[i..].windows(2).position(|w| w == b"\r\n") else {
            return Err(ServeError::Parse);
        };
        let line = &src[i..i + nl];
        if line.contains(&b';') {
            return Err(ServeError::Parse);
        }
        let size = usize::from_str_radix(std::str::from_utf8(line).map_err(|_| ServeError::Parse)?, 16)
            .map_err(|_| ServeError::Parse)?;
        i += nl + 2;
        if size == 0 {
            return Ok(i + 2);
        }
        if src.len() < i + size + 2 {
            return Err(ServeError::Parse);
        }
        dst.extend_from_slice(&src[i..i + size]);
        i += size + 2;
    }
}

pub fn parse_request(buf: &[u8], max_header: usize) -> Result<ParseStatus<'_>, ServeError> {
    if scan_header_block(buf, max_header)?.is_none() {
        return Ok(ParseStatus::Partial);
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
    let (path, query, uri_host) = normalize_target(method, target)?;
    let mut content_length = 0usize;
    let mut saw_cl = false;
    let mut saw_te = false;
    let mut chunked = false;
    let version = req.version.unwrap_or(0);
    let mut keepalive = version >= 1;
    let mut upgrade = false;
    let mut host_hdr: Option<&str> = None;
    let mut pairs = Vec::with_capacity(req.headers.len());
    for h in req.headers.iter() {
        let val = std::str::from_utf8(h.value).map_err(|_| ServeError::Parse)?;
        if h.name.eq_ignore_ascii_case("content-length") {
            if saw_cl {
                return Err(ServeError::Parse);
            }
            content_length = val.trim().parse().map_err(|_| ServeError::Parse)?;
            saw_cl = true;
        }
        if h.name.eq_ignore_ascii_case("transfer-encoding") {
            if saw_te || !te_is_chunked_only(val) {
                return Err(ServeError::Parse);
            }
            saw_te = true;
            chunked = true;
        }
        if h.name.eq_ignore_ascii_case("connection") {
            if connection_has(val, "close") {
                keepalive = false;
            }
            if connection_has(val, "keep-alive") {
                keepalive = true;
            }
            if connection_has(val, "upgrade") {
                upgrade = true;
            }
        }
        if h.name.eq_ignore_ascii_case("upgrade") {
            upgrade = true;
        }
        if h.name.eq_ignore_ascii_case("host") {
            if host_hdr.is_some() {
                return Err(ServeError::Parse);
            }
            host_hdr = Some(val);
        }
        pairs.push((h.name, val));
    }
    if saw_cl && saw_te {
        return Err(ServeError::Parse);
    }
    // HTTP/1.1 requires Host (RFC 9112). HTTP/1.0 does not; a request
    // with no Host is served (200 on a matching rule).
    if version >= 1 && host_hdr.is_none() {
        return Err(ServeError::Parse);
    }
    if let Some(uh) = uri_host {
        if let Some(hh) = host_hdr {
            if !host_without_port(uh).eq_ignore_ascii_case(host_without_port(hh.trim())) {
                return Err(ServeError::Parse);
            }
        }
    }
    let mut wire_end = n + content_length;
    if chunked {
        match measure_chunked(&buf[n..])? {
            None => return Ok(ParseStatus::Partial),
            Some((decoded, wire)) => {
                content_length = decoded;
                wire_end = n + wire;
            }
        }
    }
    Ok(ParseStatus::Complete(Parsed {
        method,
        path,
        query,
        headers: pairs,
        header_end: n,
        wire_end,
        content_length,
        chunked,
        keepalive,
        upgrade,
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
                depth = depth.checked_add(1).ok_or(ServeError::JsonDepth)?;
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

    fn complete(raw: &[u8]) -> Parsed<'_> {
        match parse_request(raw, 16384).unwrap() {
            ParseStatus::Complete(p) => p,
            ParseStatus::Partial => panic!("expected complete"),
        }
    }

    fn err(raw: &[u8]) {
        assert!(
            matches!(parse_request(raw, 16384), Err(ServeError::Parse)),
            "expected parse error for {:?}",
            std::str::from_utf8(raw)
        );
    }

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
        let p = complete(raw);
        assert_eq!(p.path, "/api/health");
        assert_eq!(p.method, Method::Get);
        assert!(p.keepalive);
        assert_eq!(p.content_length, 0);
        assert_eq!(p.wire_end, p.header_end);
    }

    #[test]
    fn cl_te_both() {
        err(b"POST / HTTP/1.1\r\nHost: x\r\nContent-Length: 4\r\nTransfer-Encoding: chunked\r\n\r\nWAIT");
    }

    #[test]
    fn duplicate_cl() {
        err(b"POST / HTTP/1.1\r\nHost: x\r\nContent-Length: 4\r\nContent-Length: 4\r\n\r\nWAIT");
    }

    #[test]
    fn te_chunked_junk() {
        err(b"POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked, identity\r\n\r\n0\r\n\r\n");
    }

    #[test]
    fn obs_fold() {
        err(b"GET / HTTP/1.1\r\nHost: x\r\nX-Foo:\r\n bar\r\n\r\n");
    }

    #[test]
    fn abs_uri_path_is_origin() {
        let p = complete(b"GET http://evil/admin HTTP/1.1\r\nHost: evil\r\n\r\n");
        assert_eq!(p.path, "/admin");
        let p = complete(b"GET http://127.0.0.1/ HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
        assert_eq!(p.path, "/");
        assert!(!p.keepalive);
    }

    #[test]
    fn host_mismatch() {
        err(b"GET http://evil/admin HTTP/1.1\r\nHost: good\r\n\r\n");
    }

    #[test]
    fn tab_in_name() {
        err(b"GET / HTTP/1.1\r\nHost: x\r\nX-Foo\tBar: 1\r\n\r\n");
    }

    #[test]
    fn chunk_ext_smuggle() {
        err(b"POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n4;\nGET / HTTP/1.1\r\n\r\n0\r\n\r\n");
    }

    #[test]
    fn chunked_ok() {
        let p = complete(b"POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nWAIT\r\n0\r\n\r\n");
        assert!(p.chunked);
        assert_eq!(p.content_length, 4);
        let mut body = Vec::new();
        decode_chunked_into(p_header_body(), &mut body).ok();
        fn p_header_body() -> &'static [u8] {
            b"4\r\nWAIT\r\n0\r\n\r\n"
        }
        assert_eq!(&body, b"WAIT");
    }

    #[test]
    fn upgrade_flag() {
        let p = complete(b"GET / HTTP/1.1\r\nHost: x\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\r\n");
        assert!(p.upgrade);
    }

    #[test]
    fn partial_headers_are_partial() {
        let raw = b"GET / HTTP/1.1\r\nHost: x\r\n";
        match parse_request(raw, 16384).unwrap() {
            ParseStatus::Partial => {}
            ParseStatus::Complete(_) => panic!("expected partial"),
        }
    }

    #[test]
    fn cl_plus_te_is_error() {
        let raw =
            b"POST / HTTP/1.1\r\nHost: x\r\nContent-Length: 4\r\nTransfer-Encoding: chunked\r\n\r\nWAIT";
        assert!(matches!(parse_request(raw, 16384), Err(ServeError::Parse)));
    }

    #[test]
    fn http11_missing_host_is_error() {
        err(b"GET / HTTP/1.1\r\n\r\n");
    }

    #[test]
    fn http10_no_host_is_ok() {
        let p = complete(b"GET / HTTP/1.0\r\n\r\n");
        assert_eq!(p.path, "/");
        assert!(!p.keepalive);
    }

    #[test]
    fn cl_non_numeric_is_error() {
        err(b"POST / HTTP/1.1\r\nHost: x\r\nContent-Length: abc\r\n\r\n");
    }

    #[test]
    fn cl_negative_is_error() {
        err(b"POST / HTTP/1.1\r\nHost: x\r\nContent-Length: -1\r\n\r\n");
    }

    #[test]
    fn te_gzip_chunked_is_error() {
        err(b"POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: gzip, chunked\r\n\r\n0\r\n\r\n");
    }

    #[test]
    fn header_line_over_max_is_error() {
        let mut raw = b"GET / HTTP/1.1\r\nHost: x\r\nX: ".to_vec();
        raw.extend(std::iter::repeat_n(b'a', 100));
        raw.extend_from_slice(b"\r\n\r\n");
        assert!(matches!(parse_request(&raw, 64), Err(ServeError::Parse)));
    }
}
