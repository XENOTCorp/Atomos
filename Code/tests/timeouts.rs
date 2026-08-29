//! Header / body / idle / module timeouts are enforced, not just declared.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::{Duration, Instant};

use atomos::config::Config;
use atomos::engine::{self, EngineKind};
use atomos::io::{CacheDirective, In, Out, OutBody};
use atomos::module::{Handler, Module};
use atomos::rules::Ruleset;
use atomos::static_router;
use atomos::status::Status;
use bytes::Bytes;

struct Sleeper(Duration);

impl Module for Sleeper {
    fn name(&self) -> &'static str {
        "sleeper"
    }
    fn handle(&self, _req: &In<'_>) -> Result<Out, atomos::error::ServeError> {
        std::thread::sleep(self.0);
        Ok(Out {
            status: Status::OK,
            reason: None,
            headers: vec![],
            body: OutBody::Raw(Bytes::from_static(b"slow")),
            cache: CacheDirective::No,
            flags: atomos::flags::FlagSet::empty(),
        })
    }
}

async fn boot(extra: &str, rules: &[u8]) -> (u16, tempfile::TempDir, Arc<atomos::route::Router>) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("index.html"), b"ok").unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let extra = extra.trim().trim_start_matches(',');
    let cfg = Config::from_json(
        format!(
            r#"{{"bind":"127.0.0.1:{port}","static_root":"{root}","memory_cap_bytes":67108864,"engine":"epoll","workers":1,"http2":false,"http3":false,{extra}}}"#,
            root = dir.path().display(),
        )
        .as_bytes(),
    )
    .unwrap();
    let rules = Ruleset::parse(rules).unwrap();
    let (router, ctx, _) = static_router(cfg, rules);
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
    (port, dir, r2)
}

fn status_line(buf: &[u8]) -> &str {
    std::str::from_utf8(buf)
        .unwrap_or("")
        .split("\r\n")
        .next()
        .unwrap_or("")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn slowloris_headers() {
    let (port, _dir, _) = boot(
        r#""header_timeout_ms":800,"body_timeout_ms":5000,"idle_timeout_ms":5000"#,
        br#"{"rules":[{"id":"s","module":"static","methods":["GET"],"include":["/*"],"exclude":[]}]}"#,
    )
    .await;
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(4))).unwrap();
    s.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
    s.write_all(b"GET / HTTP/1.1\r\n").unwrap();
    let t0 = Instant::now();
    let mut i = 0u8;
    loop {
        if t0.elapsed() > Duration::from_secs(3) {
            panic!("slowloris still open after 3s");
        }
        std::thread::sleep(Duration::from_millis(200));
        i = i.wrapping_add(1);
        if s.write_all(b"x").is_err() {
            break;
        }
        let mut tmp = [0u8; 32];
        match s.read(&mut tmp) {
            Ok(0) => break,
            Ok(_) => break,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(_) => break,
        }
    }
    assert!(
        t0.elapsed() < Duration::from_secs(3),
        "closed in {:?}",
        t0.elapsed()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn slow_body() {
    let (port, _dir, _) = boot(
        r#""header_timeout_ms":5000,"body_timeout_ms":400,"idle_timeout_ms":5000"#,
        br#"{"rules":[{"id":"s","module":"static","methods":["GET","POST"],"include":["/*"],"exclude":[]}]}"#,
    )
    .await;
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    s.write_all(b"POST / HTTP/1.1\r\nHost: x\r\nContent-Length: 8\r\n\r\nX")
        .unwrap();
    let mut body = Vec::new();
    let _ = s.read_to_end(&mut body);
    let sl = status_line(&body);
    assert!(
        sl.starts_with("HTTP/1.1 408"),
        "expected 408, got {sl:?} {}",
        String::from_utf8_lossy(&body)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn idle_keepalive() {
    let (port, _dir, _) = boot(
        r#""header_timeout_ms":5000,"body_timeout_ms":5000,"idle_timeout_ms":400"#,
        br#"{"rules":[{"id":"s","module":"static","methods":["GET"],"include":["/*"],"exclude":[]}]}"#,
    )
    .await;
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    s.write_all(b"GET / HTTP/1.1\r\nHost: x\r\nConnection: keep-alive\r\n\r\n")
        .unwrap();
    let mut tmp = [0u8; 512];
    let n = s.read(&mut tmp).unwrap();
    assert!(status_line(&tmp[..n]).starts_with("HTTP/1.1 200"));
    std::thread::sleep(Duration::from_millis(600));
    let n = s.read(&mut tmp).unwrap_or(0);
    assert_eq!(n, 0, "idle keep-alive must EOF");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn module_timeout() {
    let (port, _dir, router) = boot(
        r#""module_timeout_ms":100,"header_timeout_ms":5000,"body_timeout_ms":5000,"idle_timeout_ms":5000"#,
        br#"{"rules":[{"id":"z","module":"sleeper","methods":["GET"],"include":["/sleep"],"exclude":[]},{"id":"s","module":"static","methods":["GET"],"include":["/*"],"exclude":["/sleep"]}]}"#,
    )
    .await;
    router.insert(
        "sleeper",
        Handler::Sync(Arc::new(Sleeper(Duration::from_millis(300)))),
    );
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    s.write_all(b"GET /sleep HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
        .unwrap();
    let mut body = Vec::new();
    let _ = s.read_to_end(&mut body);
    let sl = status_line(&body);
    assert!(
        sl.starts_with("HTTP/1.1 504"),
        "expected 504 after over-budget handle, got {sl:?}"
    );
}

#[cfg(feature = "wasm")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wasm_fuel() {
    let wasm = include_bytes!("fixtures/loop.wasm");
    let dir = tempfile::tempdir().unwrap();
    let wasm_path = dir.path().join("loop.wasm");
    std::fs::write(&wasm_path, wasm).unwrap();
    std::fs::write(dir.path().join("index.html"), b"ok").unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let cfg = Config::from_json(
        format!(
            r#"{{"bind":"127.0.0.1:{port}","static_root":"{}","memory_cap_bytes":67108864,"engine":"epoll","workers":1,"http2":false,"http3":false,"wasm_fuel":20000,"module_timeout_ms":8000}}"#,
            dir.path().display()
        )
        .as_bytes(),
    )
    .unwrap();
    let rules = Ruleset::parse(
        br#"{"rules":[{"id":"w","module":"loop","methods":["GET"],"include":["/loop"],"exclude":[]},{"id":"s","module":"static","methods":["GET"],"include":["/*"],"exclude":["/loop"]}]}"#,
    )
    .unwrap();
    let (router, ctx, _) = static_router(cfg, rules);
    let m = atomos::plugin::wasm::load_limited(&wasm_path, 20_000, 16 * 1024 * 1024).unwrap();
    router.insert("loop", Handler::Sync(m));
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
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(8))).unwrap();
    s.write_all(b"GET /loop HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
        .unwrap();
    let mut body = Vec::new();
    let _ = s.read_to_end(&mut body);
    let sl = status_line(&body);
    assert!(
        sl.starts_with("HTTP/1.1 504"),
        "fuel exhaust is 504, got {sl:?}"
    );
    let mut s2 = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s2.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    s2.write_all(b"GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
        .unwrap();
    let mut b2 = Vec::new();
    let _ = s2.read_to_end(&mut b2);
    assert!(
        status_line(&b2).starts_with("HTTP/1.1 200"),
        "worker still accepts after wasm 504: {}",
        status_line(&b2)
    );
}
