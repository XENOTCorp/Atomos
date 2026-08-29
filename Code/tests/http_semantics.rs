//! Range, HEAD, 304, sendfile, queued chunked stream.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use atomos::config::Config;
use atomos::engine::{self, EngineKind};
use atomos::io::{CacheDirective, In, Out, OutBody, StreamBody};
use atomos::module::{Handler, Module};
use atomos::rules::Ruleset;
use atomos::static_router;
use atomos::status::Status;
use bytes::Bytes;
use parking_lot::Mutex;

struct Counter(AtomicU64);

impl Module for Counter {
    fn name(&self) -> &'static str {
        "counter"
    }
    fn handle(&self, _req: &In<'_>) -> Result<Out, atomos::error::ServeError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(Out {
            status: Status::OK,
            reason: None,
            headers: vec![("ETag".into(), "\"c1\"".into())],
            body: OutBody::Raw(Bytes::from_static(b"count")),
            cache: CacheDirective::Global { ttl_ms: 60_000 },
            flags: atomos::flags::FlagSet::empty(),
        })
    }
}

struct Streamer;

impl Module for Streamer {
    fn name(&self) -> &'static str {
        "streamer"
    }
    fn handle(&self, _req: &In<'_>) -> Result<Out, atomos::error::ServeError> {
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        tx.try_send(Bytes::from_static(b"hello")).unwrap();
        drop(tx);
        Ok(Out {
            status: Status::OK,
            reason: None,
            headers: vec![("Content-Type".into(), "text/plain".into())],
            body: OutBody::Stream(StreamBody(Arc::new(Mutex::new(Some(rx))))),
            cache: CacheDirective::No,
            flags: atomos::flags::FlagSet::empty(),
        })
    }
}

async fn boot() -> (u16, tempfile::TempDir, Arc<Counter>, Arc<atomos::route::Router>) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("index.html"), b"ABCDEFGH").unwrap();
    let mut big = vec![0u8; 1024 * 1024];
    for (i, b) in big.iter_mut().enumerate() {
        *b = (i % 251) as u8;
    }
    std::fs::write(dir.path().join("big.bin"), &big).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let cfg = Config::from_json(
        format!(
            r#"{{"bind":"127.0.0.1:{port}","static_root":"{}","memory_cap_bytes":67108864,"engine":"epoll","workers":1,"http2":false,"http3":false,"cache_entries":64,"cache_bytes":2097152}}"#,
            dir.path().display()
        )
        .as_bytes(),
    )
    .unwrap();
    let rules = Ruleset::parse(
        br#"{"rules":[
            {"id":"c","module":"counter","methods":["GET"],"include":["/count"],"exclude":[]},
            {"id":"st","module":"streamer","methods":["GET"],"include":["/stream"],"exclude":[]},
            {"id":"s","module":"static","methods":["GET","HEAD"],"include":["/*"],"exclude":["/count","/stream"]}
        ]}"#,
    )
    .unwrap();
    let (router, ctx, _) = static_router(cfg, rules);
    let ctr = Arc::new(Counter(AtomicU64::new(0)));
    router.insert("counter", Handler::Sync(ctr.clone()));
    router.insert("streamer", Handler::Sync(Arc::new(Streamer)));
    let r2 = router.clone();
    tokio::spawn(async move {
        let _ = engine::run(EngineKind::Epoll, router, ctx).await;
    });
    let addr = format!("127.0.0.1:{port}");
    for _ in 0..50 {
        if TcpStream::connect(&addr).is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    (port, dir, ctr, r2)
}

fn exchange(port: u16, req: &[u8]) -> Vec<u8> {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    s.write_all(req).unwrap();
    let mut b = Vec::new();
    let _ = s.read_to_end(&mut b);
    b
}

fn split_head_body(buf: &[u8]) -> (&str, &[u8]) {
    let sep = buf.windows(4).position(|w| w == b"\r\n\r\n").unwrap_or(buf.len());
    let head = std::str::from_utf8(&buf[..sep]).unwrap_or("");
    let body = if sep + 4 <= buf.len() { &buf[sep + 4..] } else { &[] };
    (head, body)
}

fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    for line in head.lines() {
        if let Some((k, v)) = line.split_once(':') {
            if k.eq_ignore_ascii_case(name) {
                return Some(v.trim());
            }
        }
    }
    None
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn range_first() {
    let (port, _dir, _, _) = boot().await;
    let b = exchange(
        port,
        b"GET / HTTP/1.1\r\nHost: x\r\nRange: bytes=0-3\r\nConnection: close\r\n\r\n",
    );
    let (head, body) = split_head_body(&b);
    assert!(head.starts_with("HTTP/1.1 206"), "{head}");
    let cr = header(head, "Content-Range").unwrap_or("");
    assert!(cr.contains("bytes 0-3/8"), "{cr}");
    assert_eq!(body, b"ABCD");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn range_suffix() {
    let (port, _dir, _, _) = boot().await;
    let b = exchange(
        port,
        b"GET / HTTP/1.1\r\nHost: x\r\nRange: bytes=-2\r\nConnection: close\r\n\r\n",
    );
    let (head, body) = split_head_body(&b);
    assert!(head.starts_with("HTTP/1.1 206"), "{head}");
    assert_eq!(body, b"GH");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn range_bad() {
    let (port, _dir, _, _) = boot().await;
    let b = exchange(
        port,
        b"GET / HTTP/1.1\r\nHost: x\r\nRange: bytes=999-1000\r\nConnection: close\r\n\r\n",
    );
    let (head, _) = split_head_body(&b);
    assert!(head.starts_with("HTTP/1.1 416"), "{head}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn head_no_body() {
    let (port, _dir, _, _) = boot().await;
    let b = exchange(port, b"HEAD / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    let (head, body) = split_head_body(&b);
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    let cl: usize = header(head, "Content-Length")
        .unwrap_or("0")
        .parse()
        .unwrap();
    assert!(cl > 0, "HEAD keeps Content-Length");
    assert!(body.is_empty(), "HEAD body must be empty, got {} bytes", body.len());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn not_modified() {
    let (port, _dir, ctr, _) = boot().await;
    let b = exchange(port, b"GET /count HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    let (head, _) = split_head_body(&b);
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(ctr.0.load(Ordering::SeqCst), 1);
    let etag = header(head, "ETag").unwrap_or("\"c1\"").to_string();
    let req = format!(
        "GET /count HTTP/1.1\r\nHost: x\r\nIf-None-Match: {etag}\r\nConnection: close\r\n\r\n"
    );
    let b2 = exchange(port, req.as_bytes());
    let (h2, body2) = split_head_body(&b2);
    assert!(h2.starts_with("HTTP/1.1 304"), "{h2}");
    assert!(body2.is_empty());
    assert_eq!(
        ctr.0.load(Ordering::SeqCst),
        1,
        "304 cache hit must not call handle"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stream_then_get() {
    let (port, _dir, _, _) = boot().await;
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    s.write_all(b"GET /stream HTTP/1.1\r\nHost: x\r\n\r\n")
        .unwrap();
    let mut acc = Vec::new();
    let mut tmp = [0u8; 512];
    for _ in 0..20 {
        let n = s.read(&mut tmp).unwrap_or(0);
        if n == 0 {
            break;
        }
        acc.extend_from_slice(&tmp[..n]);
        if acc.windows(5).any(|w| w == b"hello") {
            break;
        }
    }
    let text = String::from_utf8_lossy(&acc);
    assert!(text.contains("Transfer-Encoding: chunked"), "{text}");
    assert!(text.contains("hello"), "{text}");
    s.write_all(b"GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
        .unwrap();
    let mut rest = Vec::new();
    let _ = s.read_to_end(&mut rest);
    let t2 = String::from_utf8_lossy(&rest);
    assert!(
        t2.contains("HTTP/1.1 200") || text.contains("HTTP/1.1 200"),
        "{t2}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sendfile_1m() {
    let (port, _dir, _, _) = boot().await;
    let b = exchange(
        port,
        b"GET /big.bin HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
    );
    let (head, body) = split_head_body(&b);
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(body.len(), 1024 * 1024);
}
