//! Length-prefixed `atomos-keyd` frames. No serde on the datapath.

/// `kind` for sign (payload = digest / rustls `Signer::sign` message).
pub const KIND_SIGN: u8 = 1;

/// Max `n` (bytes after the u32be length). Larger frames are refused.
pub const MAX_FRAME: usize = 65536;

/// `u32be n | u8 kind | [n-1 bytes payload]`.
pub fn encode_req(kind: u8, payload: &[u8]) -> Vec<u8> {
    let n = 1u32.saturating_add(payload.len() as u32);
    let mut out = Vec::with_capacity(4 + 1 + payload.len());
    out.extend_from_slice(&n.to_be_bytes());
    out.push(kind);
    out.extend_from_slice(payload);
    out
}

/// Inverse of [`encode_req`]. `None` if truncated, `n == 0`, or `n > MAX_FRAME`.
pub fn decode_req(buf: &[u8]) -> Option<(u8, &[u8])> {
    let n = frame_len(buf)?;
    if n == 0 {
        return None;
    }
    let total = 4 + n;
    Some((buf[4], &buf[5..total]))
}

/// `u32be n | payload`.
pub fn encode_rep(payload: &[u8]) -> Vec<u8> {
    let n = payload.len() as u32;
    let mut out = Vec::with_capacity(4 + payload.len());
    out.extend_from_slice(&n.to_be_bytes());
    out.extend_from_slice(payload);
    out
}

/// Inverse of [`encode_rep`]. `None` if truncated or `n > MAX_FRAME`.
pub fn decode_rep(buf: &[u8]) -> Option<&[u8]> {
    let n = frame_len(buf)?;
    Some(&buf[4..4 + n])
}

fn frame_len(buf: &[u8]) -> Option<usize> {
    if buf.len() < 4 {
        return None;
    }
    let n = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    if n > MAX_FRAME {
        return None;
    }
    let total = 4usize.checked_add(n)?;
    if buf.len() < total {
        return None;
    }
    Some(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_request_roundtrip_bytes() {
        let d = [7u8; 32];
        let b = super::encode_req(1, &d);
        let (k, p) = super::decode_req(&b).unwrap();
        assert_eq!(k, 1);
        assert_eq!(p, d);
    }

    #[test]
    fn sign_reply_roundtrip_bytes() {
        let sig = [9u8; 64];
        let b = encode_rep(&sig);
        assert_eq!(decode_rep(&b).unwrap(), sig.as_slice());
    }

    #[test]
    fn decode_req_fail_closed_truncated_or_empty() {
        let b = encode_req(KIND_SIGN, &[7u8; 32]);
        assert!(decode_req(&[]).is_none());
        assert!(decode_req(&b[..3]).is_none());
        assert!(decode_req(&b[..4]).is_none());
        assert!(decode_req(&b[..b.len() - 1]).is_none());
        assert!(decode_req(&[0, 0, 0, 0]).is_none());
    }
}
