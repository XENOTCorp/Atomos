//! Access log after encode. Effect adapter; not on the cache-hit predicate.
//! Domain: one CLF-ish line per response. No `format!` on the hot path.

use std::cell::RefCell;
use std::io::{self, Write};

use crate::io::Method;
use crate::num::{u16_to_slice, usize_to_slice};

thread_local! {
    /// When set, `emit` appends here instead of stderr (unit tests).
    static CAPTURE: RefCell<Option<Vec<u8>>> = const { RefCell::new(None) };
}

/// Write one line: `METHOD path status body_len\n` into `dst` (itoa, no format!).
pub fn line(method: Method, path: &str, status: u16, body_len: usize, dst: &mut Vec<u8>) {
    dst.extend_from_slice(method.as_str().as_bytes());
    dst.push(b' ');
    // Bound path copy so logging cannot grow without limit from a hostile URI.
    const MAX_PATH: usize = 2048;
    let p = path.as_bytes();
    let n = p.len().min(MAX_PATH);
    dst.extend_from_slice(&p[..n]);
    dst.push(b' ');
    let mut nb = [0u8; 20];
    let k = u16_to_slice(status, &mut nb);
    dst.extend_from_slice(&nb[..k]);
    dst.push(b' ');
    let k = usize_to_slice(body_len, &mut nb);
    dst.extend_from_slice(&nb[..k]);
    dst.push(b'\n');
}

/// Build a line then write to the capture buffer or stderr.
pub fn emit(method: Method, path: &str, status: u16, body_len: usize) {
    let mut buf = Vec::with_capacity(64 + path.len().min(2048));
    line(method, path, status, body_len, &mut buf);
    CAPTURE.with(|c| {
        if let Some(v) = c.borrow_mut().as_mut() {
            v.extend_from_slice(&buf);
        } else {
            let _ = io::stderr().write_all(&buf);
        }
    });
}

#[cfg(test)]
pub fn capture_start() {
    CAPTURE.with(|c| *c.borrow_mut() = Some(Vec::new()));
}

#[cfg(test)]
pub fn capture_take() -> Vec<u8> {
    CAPTURE.with(|c| c.borrow_mut().take().unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_contains_path_and_status() {
        let mut dst = Vec::new();
        line(Method::Get, "/metrics", 200, 12, &mut dst);
        let s = std::str::from_utf8(&dst).unwrap();
        assert!(s.contains("/metrics"), "{s}");
        assert!(s.contains("200"), "{s}");
        assert!(s.starts_with("GET "), "{s}");
    }
}
