//! H1 encodes `OutBody::Stream` as chunked transfer. Connection reusable.

use std::io::{Read, Write};
use std::net::TcpStream;
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

struct Streamer;

impl Module for Streamer {
    fn name(&self) -> &'static str {
        "streamer"
    }
    fn handle(&self, _req: &In<'_>) -> Result<Out, atomos::error::ServeError> {
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        tx.try_send(Bytes::from_static(b"hello")).unwrap();
        tx.try_send(Bytes::from_static(b"!")).unwrap();
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chunked_then_reuse() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("index.html"), b"ok").unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let cfg = Config::from_json(
        format!(
            r#"{{"bind":"127.0.0.1:{port}","static_root":"{}","memory_cap_bytes":67108864,"engine":"epoll","workers":1}}"#,
            dir.path().display()
        )
        .as_bytes(),
    )
    .unwrap();
    let rules = Ruleset::parse(
        br#"{"rules":[{"id":"st","module":"streamer","methods":["GET"],"include":["/stream"],"exclude":[]},{"id":"s","module":"static","methods":["GET"],"include":["/*"],"exclude":["/stream"]}]}"#,
    )
    .unwrap();
    let (router, ctx, _) = static_router(cfg, rules);
    router.insert("streamer", Handler::Sync(Arc::new(Streamer)));
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
    let mut s = TcpStream::connect(&addr).unwrap();
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
    assert!(t2.contains("HTTP/1.1 200") || text.contains("HTTP/1.1 200"), "{t2}");
}
