//! Encode `Out` to one HTTP/1.1 response buffer. Criticality C1.

use crate::io::Out;
use crate::num::{u16_to_slice, usize_to_slice};

pub fn encode_response(out: &Out, dst: &mut Vec<u8>) {
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
    let body = out.body.as_bytes();
    dst.extend_from_slice(b"Content-Length: ");
    let n = usize_to_slice(body.len(), &mut nb);
    dst.extend_from_slice(&nb[..n]);
    dst.extend_from_slice(b"\r\nConnection: keep-alive\r\nX-Content-Type-Options: nosniff\r\nX-Frame-Options: DENY\r\nReferrer-Policy: no-referrer\r\n");
    for (k, v) in &out.headers {
        dst.extend_from_slice(k.as_bytes());
        dst.extend_from_slice(b": ");
        dst.extend_from_slice(v.as_bytes());
        dst.extend_from_slice(b"\r\n");
    }
    dst.extend_from_slice(b"\r\n");
    dst.extend_from_slice(body);
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
}
