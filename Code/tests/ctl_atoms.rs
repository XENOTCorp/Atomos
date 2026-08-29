//! server.drain and audit.append.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

use atomos::atom::AtomCtx;
use atomos::config::Config;
use atomos::engine::{self, EngineKind};
use atomos::io::{CacheDirective, In, Out, OutBody};
use atomos::module::{Handler, Module};
use atomos::rules::Ruleset;
use atomos::static_router;
use atomos::status::Status;
use bytes::Bytes;
use serde_json::json;

struct Slow;

impl Module for Slow {
    fn name(&self) -> &'static str {
        "slow"
    }
    fn handle(&self, _req: &In<'_>) -> Result<Out, atomos::error::ServeError> {
        std::thread::sleep(Duration::from_millis(250));
        Ok(Out {
            status: Status::OK,
            reason: None,
            headers: vec![],
            body: OutBody::Raw(Bytes::from_static(b"done")),
            cache: CacheDirective::No,
            flags: atomos::flags::FlagSet::empty(),
        })
    }
}

#[test]
fn audit_append_writes_line() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("audit.log");
    let mut ctx = AtomCtx::test();
    ctx.audit_path = path.clone();
    ctx.run(
        "audit.append",
        json!({"atom":"audit.append","key_id":"k","old_hash":"a","new_hash":"b"}),
    )
    .unwrap();
    ctx.run("cache.purge", json!({})).unwrap();
    let raw = std::fs::read_to_string(&path).unwrap();
    assert!(raw.contains("audit.append"), "{raw}");
    assert!(raw.contains("cache.purge"), "{raw}");
    for line in raw.lines() {
        let parts: Vec<_> = line.split(',').map(|s| s.trim()).collect();
        assert!(parts.len() >= 5, "{line}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn server_drain_finishes_in_flight_refuses_new() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("index.html"), b"ok").unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let cfg = Config::from_json(
        format!(
            r#"{{"bind":"127.0.0.1:{port}","static_root":"{}","memory_cap_bytes":67108864,"engine":"epoll","workers":1,"http2":false,"http3":false,"worker_shutdown_timeout_ms":1500}}"#,
            dir.path().display()
        )
        .as_bytes(),
    )
    .unwrap();
    let rules = Ruleset::parse(
        br#"{"rules":[{"id":"z","module":"slow","methods":["GET"],"include":["/slow"],"exclude":[]},{"id":"s","module":"static","methods":["GET"],"include":["/*"],"exclude":["/slow"]}]}"#,
    )
    .unwrap();
    let (router, ctx, _) = static_router(cfg, rules);
    router.insert("slow", Handler::Sync(Arc::new(Slow)));
    let ctx2 = ctx.clone();
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
    let h = std::thread::spawn(move || {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        s.write_all(b"GET /slow HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
            .unwrap();
        let mut b = Vec::new();
        let _ = s.read_to_end(&mut b);
        b
    });
    std::thread::sleep(Duration::from_millis(40));
    ctx2.run("server.drain", json!({})).unwrap();
    let body = h.join().unwrap();
    let sl = std::str::from_utf8(&body).unwrap_or("").split("\r\n").next().unwrap_or("");
    assert!(
        sl.starts_with("HTTP/1.1 200"),
        "in-flight GET must finish, got {sl:?}"
    );
    std::thread::sleep(Duration::from_millis(50));
    let mut refused = false;
    for _ in 0..20 {
        match TcpStream::connect_timeout(
            &format!("127.0.0.1:{port}").parse().unwrap(),
            Duration::from_millis(100),
        ) {
            Err(_) => {
                refused = true;
                break;
            }
            Ok(mut s) => {
                s.set_read_timeout(Some(Duration::from_millis(200))).ok();
                let mut buf = [0u8; 8];
                match s.read(&mut buf) {
                    Ok(0) => {
                        refused = true;
                        break;
                    }
                    Err(_) => {
                        refused = true;
                        break;
                    }
                    Ok(_) => {}
                }
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(refused, "new SYN must be refused after drain");
}
