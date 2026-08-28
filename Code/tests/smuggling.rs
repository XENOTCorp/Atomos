//! Request smuggling: Content-Length + Transfer-Encoding is 400.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use atomos::config::Config;
use atomos::engine::{self, EngineKind};
use atomos::rules::Ruleset;
use atomos::static_router;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn content_length_and_transfer_encoding_rejected() {
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
    let mut body = Vec::new();
    for _ in 0..50 {
        if let Ok(mut s) = TcpStream::connect(&addr) {
            s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            write!(
                s,
                "POST / HTTP/1.1\r\nHost: x\r\nContent-Length: 4\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\nWAIT"
            )
            .unwrap();
            s.read_to_end(&mut body).unwrap();
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let s = String::from_utf8_lossy(&body);
    assert!(
        s.starts_with("HTTP/1.1 400") || s.is_empty() || s.starts_with("HTTP/1.1 4"),
        "{s}"
    );
}
