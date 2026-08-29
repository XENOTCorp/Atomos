//! HTTP/1.1 framing corpus. One named case per attack.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use atomos::config::Config;
use atomos::engine::{self, EngineKind};
use atomos::rules::Ruleset;
use atomos::static_router;

async fn spawn_static() -> (u16, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("index.html"), b"ok").unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let cfg = Config::from_json(
        format!(
            r#"{{"bind":"127.0.0.1:{port}","static_root":"{}","memory_cap_bytes":67108864,"engine":"epoll","workers":1,"http2":false,"http3":false}}"#,
            dir.path().display()
        )
        .as_bytes(),
    )
    .unwrap();
    let rules = Ruleset::parse(
        br#"{"rules":[{"id":"s","module":"static","methods":["GET","POST","HEAD"],"include":["/*"],"exclude":[]}]}"#,
    )
    .unwrap();
    let (router, ctx, _) = static_router(cfg, rules);
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
    (port, dir)
}

fn exchange(port: u16, req: &[u8]) -> Vec<u8> {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    s.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
    s.write_all(req).unwrap();
    let _ = s.shutdown(std::net::Shutdown::Write);
    let mut body = Vec::new();
    let _ = s.read_to_end(&mut body);
    body
}

fn status_line(body: &[u8]) -> &str {
    let s = std::str::from_utf8(body).unwrap_or("");
    s.split("\r\n").next().unwrap_or(s)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cl_te_both() {
    let (port, _dir) = spawn_static().await;
    let b = exchange(
        port,
        b"POST / HTTP/1.1\r\nHost: x\r\nContent-Length: 4\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\nWAIT",
    );
    assert!(
        status_line(&b).starts_with("HTTP/1.1 400") || b.is_empty(),
        "{}",
        status_line(&b)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn duplicate_cl() {
    let (port, _dir) = spawn_static().await;
    let b = exchange(
        port,
        b"POST / HTTP/1.1\r\nHost: x\r\nContent-Length: 4\r\nContent-Length: 5\r\nConnection: close\r\n\r\nWAIT!",
    );
    assert!(status_line(&b).starts_with("HTTP/1.1 400") || b.is_empty(), "{}", status_line(&b));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn te_chunked_junk() {
    let (port, _dir) = spawn_static().await;
    let b = exchange(
        port,
        b"POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked, identity\r\nConnection: close\r\n\r\n0\r\n\r\n",
    );
    assert!(status_line(&b).starts_with("HTTP/1.1 400") || b.is_empty(), "{}", status_line(&b));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn obs_fold() {
    let (port, _dir) = spawn_static().await;
    let b = exchange(
        port,
        b"GET / HTTP/1.1\r\nHost: x\r\nX-Foo:\r\n bar\r\nConnection: close\r\n\r\n",
    );
    assert!(status_line(&b).starts_with("HTTP/1.1 400") || b.is_empty(), "{}", status_line(&b));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn abs_uri() {
    let (port, _dir) = spawn_static().await;
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    s.write_all(b"GET http://127.0.0.1/ HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .unwrap();
    let mut body = Vec::new();
    let _ = s.read_to_end(&mut body);
    let sl = status_line(&body);
    assert!(
        sl.starts_with("HTTP/1.1 200") || sl.starts_with("HTTP/1.1 400"),
        "{sl:?} body={body:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn host_mismatch() {
    let (port, _dir) = spawn_static().await;
    let b = exchange(
        port,
        b"GET http://evil/admin HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
    );
    assert!(status_line(&b).starts_with("HTTP/1.1 400") || b.is_empty(), "{}", status_line(&b));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tab_in_name() {
    let (port, _dir) = spawn_static().await;
    let b = exchange(
        port,
        b"GET / HTTP/1.1\r\nHost: x\r\nX-Foo\tBar: 1\r\nConnection: close\r\n\r\n",
    );
    assert!(status_line(&b).starts_with("HTTP/1.1 400") || b.is_empty(), "{}", status_line(&b));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chunk_ext_smuggle() {
    let (port, _dir) = spawn_static().await;
    let b = exchange(
        port,
        b"POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n4;\nGET / HTTP/1.1\r\n\r\n0\r\n\r\n",
    );
    assert!(status_line(&b).starts_with("HTTP/1.1 400") || b.is_empty(), "{}", status_line(&b));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pipeline_two() {
    let (port, _dir) = spawn_static().await;
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    s.write_all(
        b"GET / HTTP/1.1\r\nHost: x\r\n\r\nGET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
    )
    .unwrap();
    let mut body = Vec::new();
    let _ = s.read_to_end(&mut body);
    let text = String::from_utf8_lossy(&body);
    let n = text.matches("HTTP/1.1 200").count();
    assert!(n >= 2, "expected two responses, got {n}: {text}");
}
