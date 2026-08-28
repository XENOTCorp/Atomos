//! Tight loopback HTTP/1.1 load generator. Keep-alive + optional pipeline.
//!
//!   cargo run --release --example loadgen -- 127.0.0.1:8090 /api/health 8 3 8
//!   args: bind  path  threads  seconds  pipeline
//!
//! Client only. Honors ATOMOS_REFUSE_PORTS.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn read_one(s: &mut TcpStream, buf: &mut Vec<u8>) -> std::io::Result<usize> {
    loop {
        if let Some(sep) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = &buf[..sep];
            let mut clen = 0usize;
            for line in head.split(|&b| b == b'\n') {
                let line = if line.last() == Some(&b'\r') {
                    &line[..line.len() - 1]
                } else {
                    line
                };
                if line.len() >= 15 && line[..15].eq_ignore_ascii_case(b"content-length:") {
                    let v = std::str::from_utf8(&line[15..]).unwrap_or("").trim();
                    clen = v.parse().unwrap_or(0);
                }
            }
            let total = sep + 4 + clen;
            if buf.len() >= total {
                buf.copy_within(total.., 0);
                buf.truncate(buf.len() - total);
                return Ok(clen);
            }
        }
        let mut tmp = [0u8; 4096];
        let n = s.read(&mut tmp)?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "eof",
            ));
        }
        buf.extend_from_slice(&tmp[..n]);
    }
}

fn worker(
    addr: String,
    req: Arc<Vec<u8>>,
    pipeline: usize,
    stop: Arc<AtomicBool>,
    ok: Arc<AtomicU64>,
    err: Arc<AtomicU64>,
) {
    let mut s = match TcpStream::connect(&addr) {
        Ok(s) => s,
        Err(_) => {
            err.fetch_add(1, Ordering::Relaxed);
            return;
        }
    };
    let _ = s.set_nodelay(true);
    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = s.set_write_timeout(Some(Duration::from_secs(5)));
    let mut buf = Vec::with_capacity(8192);
    let pipe = pipeline.max(1);
    while !stop.load(Ordering::Relaxed) {
        for _ in 0..pipe {
            if s.write_all(&req).is_err() {
                err.fetch_add(1, Ordering::Relaxed);
                return;
            }
        }
        for _ in 0..pipe {
            match read_one(&mut s, &mut buf) {
                Ok(_) => {
                    ok.fetch_add(1, Ordering::Relaxed);
                }
                Err(_) => {
                    err.fetch_add(1, Ordering::Relaxed);
                    return;
                }
            }
        }
    }
}

fn main() {
    let bind = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:8090".into());
    let path = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "/api/health".into());
    let threads: usize = std::env::args()
        .nth(3)
        .and_then(|s| s.parse().ok())
        .unwrap_or(8);
    let secs: u64 = std::env::args()
        .nth(4)
        .and_then(|s| s.parse().ok())
        .unwrap_or(3);
    let pipeline: usize = std::env::args()
        .nth(5)
        .and_then(|s| s.parse().ok())
        .unwrap_or(8);
    if let Some(list) = std::env::var_os("ATOMOS_REFUSE_PORTS") {
        let s = list.to_string_lossy();
        if s.split(',').any(|p| bind.ends_with(&format!(":{p}"))) {
            eprintln!("loadgen: bind is in ATOMOS_REFUSE_PORTS");
            std::process::exit(2);
        }
    }
    let req = Arc::new(
        format!("GET {path} HTTP/1.1\r\nHost: x\r\nConnection: keep-alive\r\n\r\n").into_bytes(),
    );
    let stop = Arc::new(AtomicBool::new(false));
    let ok = Arc::new(AtomicU64::new(0));
    let err = Arc::new(AtomicU64::new(0));
    let mut joins = Vec::new();
    let t0 = Instant::now();
    for _ in 0..threads {
        let addr = bind.clone();
        let req = req.clone();
        let stop = stop.clone();
        let ok = ok.clone();
        let err = err.clone();
        joins.push(std::thread::spawn(move || {
            worker(addr, req, pipeline, stop, ok, err);
        }));
    }
    std::thread::sleep(Duration::from_secs(secs));
    stop.store(true, Ordering::SeqCst);
    for j in joins {
        let _ = j.join();
    }
    let dt = t0.elapsed().as_secs_f64();
    let n_ok = ok.load(Ordering::Relaxed);
    let n_err = err.load(Ordering::Relaxed);
    let rps = n_ok as f64 / dt;
    println!(
        "bind={bind} path={path} threads={threads} pipeline={pipeline} secs={dt:.3} ok={n_ok} err={n_err} rps={rps:.0}"
    );
}
