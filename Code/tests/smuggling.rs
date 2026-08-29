//! HTTP/1.1 framing corpus. One named case per attack.
//! Empty read after write-shutdown is a close, not a 400.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use atomos::config::Config;
use atomos::engine::{self, EngineKind};
use atomos::rules::Ruleset;
use atomos::static_router;

async fn spawn_static() -> (u16, tempfile::TempDir) {
    spawn_static_json(
        r#""memory_cap_bytes":67108864,"engine":"epoll","workers":1,"http2":false,"http3":false"#,
    )
    .await
}

async fn spawn_static_json(extra: &str) -> (u16, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("index.html"), b"ok").unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let extra = extra.trim().trim_start_matches(',');
    let cfg = Config::from_json(
        format!(
            r#"{{"bind":"127.0.0.1:{port}","static_root":"{root}",{extra}}}"#,
            root = dir.path().display(),
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

fn assert_400(buf: &[u8]) {
    let sl = status_line(buf);
    assert!(sl.starts_with("HTTP/1.1 400"), "got {sl:?} body={}", sl);
    assert!(!buf.is_empty());
}

fn status_line_count(buf: &[u8]) -> usize {
    String::from_utf8_lossy(buf)
        .lines()
        .filter(|l| l.starts_with("HTTP/1."))
        .count()
}

/// 400, then the socket is not reused for a second request.
fn assert_400_no_reuse(port: u16, req: &[u8]) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    s.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
    s.write_all(req).unwrap();
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    match s.read(&mut tmp) {
        Ok(0) => {}
        Ok(n) => buf.extend_from_slice(&tmp[..n]),
        Err(_) => {}
    }
    assert_400(&buf);
    match s.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n") {
        Err(_) => {}
        Ok(()) => {
            let mut b2 = Vec::new();
            let _ = s.read_to_end(&mut b2);
            let sl = status_line(&b2);
            assert!(
                b2.is_empty() || !sl.starts_with("HTTP/1.1 200"),
                "connection reused: {sl:?}"
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cl_te_both() {
    let (port, _dir) = spawn_static().await;
    assert_400_no_reuse(
        port,
        b"POST / HTTP/1.1\r\nHost: x\r\nContent-Length: 4\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\nWAIT",
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn duplicate_cl() {
    let (port, _dir) = spawn_static().await;
    assert_400_no_reuse(
        port,
        b"POST / HTTP/1.1\r\nHost: x\r\nContent-Length: 4\r\nContent-Length: 5\r\nConnection: close\r\n\r\nWAIT!",
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn te_chunked_junk() {
    let (port, _dir) = spawn_static().await;
    assert_400_no_reuse(
        port,
        b"POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked, identity\r\nConnection: close\r\n\r\n0\r\n\r\n",
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn obs_fold() {
    let (port, _dir) = spawn_static().await;
    assert_400_no_reuse(
        port,
        b"GET / HTTP/1.1\r\nHost: x\r\nX-Foo:\r\n bar\r\nConnection: close\r\n\r\n",
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tab_in_name() {
    let (port, _dir) = spawn_static().await;
    assert_400_no_reuse(
        port,
        b"GET / HTTP/1.1\r\nHost: x\r\nX-Foo\tBar: 1\r\nConnection: close\r\n\r\n",
    );
}

/// Absolute-form is origin-form before the ruleset (`normalize_target`).
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
        sl.starts_with("HTTP/1.1 200"),
        "{sl:?} body={}",
        String::from_utf8_lossy(&body)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn host_mismatch() {
    let (port, _dir) = spawn_static().await;
    let b = exchange(
        port,
        b"GET http://evil/admin HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
    );
    assert_400(&b);
    assert_eq!(status_line_count(&b), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chunk_ext_smuggle() {
    let (port, _dir) = spawn_static().await;
    let b = exchange(
        port,
        b"POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n4;\nGET / HTTP/1.1\r\n\r\n0\r\n\r\n",
    );
    assert_400(&b);
    assert_eq!(status_line_count(&b), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pipeline_two() {
    let (port, _dir) = spawn_static().await;
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    s.write_all(
        b"GET / HTTP/1.1\r\nHost: x\r\nConnection: keep-alive\r\n\r\nGET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
    )
    .unwrap();
    let _ = s.shutdown(std::net::Shutdown::Write);
    let mut body = Vec::new();
    let _ = s.read_to_end(&mut body);
    let text = String::from_utf8_lossy(&body);
    let n = text.matches("HTTP/1.1 200").count();
    assert!(n >= 2, "expected two responses, got {n}: {text}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cl_non_numeric() {
    let (port, _dir) = spawn_static().await;
    assert_400_no_reuse(
        port,
        b"POST / HTTP/1.1\r\nHost: x\r\nContent-Length: abc\r\nConnection: close\r\n\r\n",
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cl_negative() {
    let (port, _dir) = spawn_static().await;
    assert_400_no_reuse(
        port,
        b"POST / HTTP/1.1\r\nHost: x\r\nContent-Length: -1\r\nConnection: close\r\n\r\n",
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_host() {
    let (port, _dir) = spawn_static().await;
    assert_400_no_reuse(port, b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n");
}

/// HTTP/1.0 may omit Host. Pick: 200 on a matching static rule.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http10_no_host() {
    let (port, _dir) = spawn_static().await;
    let b = exchange(port, b"GET / HTTP/1.0\r\n\r\n");
    let sl = status_line(&b);
    assert!(
        sl.starts_with("HTTP/1.1 200") || sl.starts_with("HTTP/1.0 200"),
        "HTTP/1.0 no Host is 200, got {sl:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn te_gzip_chunked() {
    let (port, _dir) = spawn_static().await;
    assert_400_no_reuse(
        port,
        b"POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: gzip, chunked\r\nConnection: close\r\n\r\n0\r\n\r\n",
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn header_oversize() {
    let (port, _dir) = spawn_static().await;
    let mut req = b"GET / HTTP/1.1\r\nHost: x\r\nX: ".to_vec();
    req.extend(std::iter::repeat_n(b'a', 20_000));
    req.extend_from_slice(b"\r\nConnection: close\r\n\r\n");
    let b = exchange(port, &req);
    assert_400(&b);
}

/// Incomplete chunked body: parse stays Partial until `body_timeout_ms`, then 408.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chunk_missing_final() {
    let (port, _dir) = spawn_static_json(
        r#""memory_cap_bytes":67108864,"engine":"epoll","workers":1,"http2":false,"http3":false,"body_timeout_ms":400,"header_timeout_ms":2000"#,
    )
    .await;
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    s.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
    s.write_all(b"POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello")
        .unwrap();
    let mut body = Vec::new();
    let _ = s.read_to_end(&mut body);
    let sl = status_line(&body);
    assert!(
        sl.starts_with("HTTP/1.1 408") || sl.starts_with("HTTP/1.1 400"),
        "incomplete chunked after body_timeout, got {sl:?} body={}",
        String::from_utf8_lossy(&body)
    );
    assert!(!body.is_empty());
}
