//! Encode `Out` to one HTTP/1.1 response buffer. Criticality C1.

use crate::io::{Out, OutBody};
use crate::num::{u16_to_slice, usize_to_slice};

pub fn encode_response(out: &Out, dst: &mut Vec<u8>) {
    encode(out, dst, false);
}

/// HEAD: same headers and Content-Length as GET. No body bytes.
pub fn encode_head(out: &Out, dst: &mut Vec<u8>) {
    encode(out, dst, true);
}

pub fn is_chunked(out: &Out) -> bool {
    matches!(out.body, OutBody::Stream(_))
}

fn encode(out: &Out, dst: &mut Vec<u8>, head: bool) {
    dst.clear();
    dst.extend_from_slice(b"HTTP/1.1 ");
    let mut nb = [0u8; 24];
    let n = u16_to_slice(out.status.as_u16(), &mut nb);
    dst.extend_from_slice(&nb[..n]);
    dst.push(b' ');
    let phrase = out
        .reason
        .as_deref()
        .unwrap_or_else(|| out.status.phrase());
    dst.extend_from_slice(phrase.as_bytes());
    dst.extend_from_slice(b"\r\n");
    let stream = matches!(out.body, OutBody::Stream(_));
    if stream {
        dst.extend_from_slice(b"Transfer-Encoding: chunked\r\n");
    } else {
        let body_len = out.body.len();
        dst.extend_from_slice(b"Content-Length: ");
        let n = usize_to_slice(body_len, &mut nb);
        dst.extend_from_slice(&nb[..n]);
        dst.extend_from_slice(b"\r\n");
    }
    dst.extend_from_slice(b"Connection: keep-alive\r\nX-Content-Type-Options: nosniff\r\nX-Frame-Options: DENY\r\nReferrer-Policy: no-referrer\r\n");
    for (k, v) in &out.headers {
        dst.extend_from_slice(k.as_bytes());
        dst.extend_from_slice(b": ");
        dst.extend_from_slice(v.as_bytes());
        dst.extend_from_slice(b"\r\n");
    }
    dst.extend_from_slice(b"\r\n");
    if !head && !stream {
        dst.extend_from_slice(out.body.as_bytes());
    }
}

/// `size\r\ndata\r\n` for one chunked body part.
pub fn append_chunk(dst: &mut Vec<u8>, data: &[u8]) {
    let mut hex = [0u8; 16];
    let n = hex_usize(data.len(), &mut hex);
    dst.extend_from_slice(&hex[..n]);
    dst.extend_from_slice(b"\r\n");
    dst.extend_from_slice(data);
    dst.extend_from_slice(b"\r\n");
}

pub fn append_chunk_end(dst: &mut Vec<u8>) {
    dst.extend_from_slice(b"0\r\n\r\n");
}

fn hex_usize(n: usize, dst: &mut [u8; 16]) -> usize {
    if n == 0 {
        dst[0] = b'0';
        return 1;
    }
    let mut x = n;
    let mut tmp = [0u8; 16];
    let mut i = 0usize;
    while x > 0 {
        tmp[i] = b"0123456789abcdef"[x & 15];
        x >>= 4;
        i += 1;
    }
    for k in 0..i {
        dst[k] = tmp[i - 1 - k];
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use crate::flags::FlagSet;
    use crate::io::{CacheDirective, Out, OutBody};
    use crate::status::Status;

    #[test]
    fn encode_one_buffer_has_length_and_body() {
        let out = Out {
            status: Status::OK,
            reason: None,
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: OutBody::Json(Bytes::from_static(br#"{"ok":true}"#)),
            cache: CacheDirective::No,
            flags: FlagSet::empty(),
        };
        let mut b = Vec::new();
        encode_response(&out, &mut b);
        let s = std::str::from_utf8(&b).unwrap();
        assert!(s.starts_with("HTTP/1.1 200 OK\r\n"), "{s}");
        assert!(s.contains("Content-Length: 11\r\n"), "{s}");
        assert!(b.ends_with(br#"{"ok":true}"#), "{s}");
        assert_eq!(s.matches("\r\n\r\n").count(), 1);
    }

    #[test]
    fn encode_head_omits_body_keeps_length() {
        let out = Out {
            status: Status::OK,
            reason: None,
            headers: vec![("Content-Type".into(), "text/plain".into())],
            body: OutBody::Raw(Bytes::from_static(b"hello")),
            cache: CacheDirective::No,
            flags: FlagSet::empty(),
        };
        let mut b = Vec::new();
        encode_head(&out, &mut b);
        let s = std::str::from_utf8(&b).unwrap();
        assert!(s.contains("Content-Length: 5\r\n"), "{s}");
        assert!(!s.contains("hello"), "{s}");
    }
}
