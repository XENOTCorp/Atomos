//! Access log after encode. Effect adapter; not on the cache-hit predicate.
//! Domain: one CLF-ish line per response. Never blocks the H1 worker.

use std::io::{self, Write};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::OnceLock;

use crate::io::Method;
use crate::num::{u16_to_slice, usize_to_slice};

static TX: OnceLock<SyncSender<Vec<u8>>> = OnceLock::new();

fn sender() -> &'static SyncSender<Vec<u8>> {
    TX.get_or_init(|| {
        let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(256);
        std::thread::Builder::new()
            .name("atomos-access-log".into())
            .spawn(move || {
                let mut err = io::stderr();
                while let Ok(line) = rx.recv() {
                    let _ = err.write_all(&line);
                }
            })
            .ok();
        tx
    })
}

/// Write one line: `METHOD path status body_len\n` into `dst` (itoa, no format!).
pub fn line(method: Method, path: &str, status: u16, body_len: usize, dst: &mut Vec<u8>) {
    dst.extend_from_slice(method.as_str().as_bytes());
    dst.push(b' ');
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

/// Build a line then try-send to the flusher. A full or blocked pipe drops the line.
pub fn emit(method: Method, path: &str, status: u16, body_len: usize) {
    let mut buf = Vec::with_capacity(64 + path.len().min(2048));
    line(method, path, status, body_len, &mut buf);
    match sender().try_send(buf) {
        Ok(()) | Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {}
    }
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
