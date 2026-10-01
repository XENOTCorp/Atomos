//! One strict chunked-framing walker shared by measurement and decoding.
//! Extensions/trailers are deliberately unsupported; sizes cannot wrap.
use crate::error::ServeError;

fn walk(
    src: &[u8],
    max_body: usize,
    mut chunk: impl FnMut(&[u8]),
) -> Result<Option<(usize, usize)>, ServeError> {
    let mut pos = 0usize;
    let mut decoded = 0usize;
    loop {
        let Some(end) = src[pos..].windows(2).position(|bytes| bytes == b"\r\n") else {
            return Ok(None);
        };
        let line = &src[pos..pos + end];
        if line.is_empty() || !line.iter().all(u8::is_ascii_hexdigit) {
            return Err(ServeError::Parse);
        }
        let size = usize::from_str_radix(
            std::str::from_utf8(line).map_err(|_| ServeError::Parse)?,
            16,
        )
        .map_err(|_| ServeError::Parse)?;
        pos += end + 2; // bounded by a CRLF already found within src
        decoded = decoded.checked_add(size).ok_or(ServeError::Parse)?;
        if decoded > max_body {
            return Err(ServeError::BodyTooLarge);
        }
        let data_end = pos.checked_add(size).ok_or(ServeError::Parse)?;
        let wire_end = data_end.checked_add(2).ok_or(ServeError::Parse)?;
        if wire_end > src.len() {
            return Ok(None);
        }
        if &src[data_end..wire_end] != b"\r\n" {
            return Err(ServeError::Parse);
        }
        if size == 0 {
            return Ok(Some((decoded, wire_end)));
        }
        chunk(&src[pos..data_end]);
        pos = wire_end;
    }
}

pub fn measure_chunked(src: &[u8]) -> Result<Option<(usize, usize)>, ServeError> {
    measure_chunked_limited(src, usize::MAX)
}

pub(super) fn measure_chunked_limited(
    src: &[u8],
    max_body: usize,
) -> Result<Option<(usize, usize)>, ServeError> {
    walk(src, max_body, |_| {})
}

/// Validate before copying, reserve once, and leave dst untouched on malformed
/// or partial input. Returns the consumed wire length, excluding pipelined data.
pub fn decode_chunked_into(src: &[u8], dst: &mut Vec<u8>) -> Result<usize, ServeError> {
    let (decoded, wire) = measure_chunked(src)?.ok_or(ServeError::Parse)?;
    dst.try_reserve(decoded).map_err(|_| ServeError::Capacity)?;
    walk(&src[..wire], decoded, |bytes| dst.extend_from_slice(bytes))?;
    Ok(wire)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn huge_sizes_and_bad_delimiters_do_not_panic_or_modify_destination() {
        for bad in [
            b"ffffffffffffffff\r\n".as_slice(),
            b"+1\r\na\r\n0\r\n\r\n",
            b"1\r\naXX0\r\n\r\n",
            b"0\r\n",
            b"0\r\nX: trailer\r\n\r\n",
        ] {
            let mut out = b"prefix".to_vec();
            assert!(decode_chunked_into(bad, &mut out).is_err(), "{bad:?}");
            assert_eq!(out, b"prefix");
        }
    }

    #[test]
    fn measurement_and_decode_stop_before_next_request() {
        let wire = b"2\r\nab\r\n1\r\nc\r\n0\r\n\r\nGET / HTTP/1.1\r\n";
        let mut out = Vec::new();
        let consumed = decode_chunked_into(wire, &mut out).unwrap();
        assert_eq!(out, b"abc");
        assert_eq!(measure_chunked(wire).unwrap(), Some((3, consumed)));
        for end in 0..consumed {
            assert!(
                measure_chunked(&wire[..end]).unwrap().is_none(),
                "prefix {end}"
            );
        }
    }

    #[test]
    fn declared_chunk_limit_is_enforced_before_payload_arrives() {
        assert!(matches!(
            measure_chunked_limited(b"100\r\n", 255),
            Err(ServeError::BodyTooLarge)
        ));
    }
}
