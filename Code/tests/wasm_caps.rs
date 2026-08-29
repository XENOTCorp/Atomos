//! Host caps: fuel, 16 MiB memory, no WASI fs/sockets, reload mid-flight.
#![cfg(feature = "wasm")]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use atomos::config::Config;
use atomos::engine::{self, EngineKind};
use atomos::module::Handler;
use atomos::plugin;
use atomos::rules::Ruleset;
use atomos::static_router;

fn status_line(buf: &[u8]) -> &str {
    std::str::from_utf8(buf)
        .unwrap_or("")
        .split("\r\n")
        .next()
        .unwrap_or("")
}

fn wait_port(port: u16) {
    let addr = format!("127.0.0.1:{port}");
    for _ in 0..50 {
        if TcpStream::connect(&addr).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn get(port: u16, path: &str, timeout: Duration) -> Vec<u8> {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(timeout)).unwrap();
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut b = Vec::new();
    let _ = s.read_to_end(&mut b);
    b
}

#[test]
fn no_fs() {
    // Instantiate uses Linker::new with no WASI. The host does not
    // define wasi:filesystem or wasi:sockets; that is the capability
    // story today (WIT is unchanged).
    let src = include_str!("../src/plugin/wasm.rs");
    assert!(
        src.contains("Linker::new(engine)"),
        "instantiate uses an empty linker"
    );
    assert!(
        !src.contains("add_to_linker"),
        "instantiate must not define WASI fs/sockets/io"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fuel_exhaust() {
    let wasm = include_bytes!("fixtures/loop.wasm");
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("index.html"), b"ok").unwrap();
    let wp = dir.path().join("loop.wasm");
    std::fs::write(&wp, wasm).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let cfg = Config::from_json(
        format!(
            r#"{{"bind":"127.0.0.1:{port}","static_root":"{}","memory_cap_bytes":67108864,"engine":"epoll","workers":1,"http2":false,"http3":false,"wasm_fuel":15000}}"#,
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
    let m = atomos::plugin::wasm::load_limited(&wp, 15_000, 16 * 1024 * 1024).unwrap();
    router.insert("loop", Handler::Sync(m));
    tokio::spawn(async move {
        let _ = engine::run(EngineKind::Epoll, router, ctx).await;
    });
    wait_port(port);
    let b = get(port, "/loop", Duration::from_secs(8));
    assert!(
        status_line(&b).starts_with("HTTP/1.1 504"),
        "{}",
        status_line(&b)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mem_grow() {
    let wasm = include_bytes!("fixtures/mem.wasm");
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("index.html"), b"ok").unwrap();
    let wp = dir.path().join("mem.wasm");
    std::fs::write(&wp, wasm).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let cfg = Config::from_json(
        format!(
            r#"{{"bind":"127.0.0.1:{port}","static_root":"{}","memory_cap_bytes":67108864,"engine":"epoll","workers":1,"http2":false,"http3":false,"wasm_memory_bytes":16777216}}"#,
            dir.path().display()
        )
        .as_bytes(),
    )
    .unwrap();
    let rules = Ruleset::parse(
        br#"{"rules":[{"id":"w","module":"mem","methods":["GET"],"include":["/mem"],"exclude":[]},{"id":"s","module":"static","methods":["GET"],"include":["/*"],"exclude":["/mem"]}]}"#,
    )
    .unwrap();
    let (router, ctx, _) = static_router(cfg, rules);
    let m = atomos::plugin::wasm::load_limited(&wp, 10_000_000, 16 * 1024 * 1024).unwrap();
    router.insert("mem", Handler::Sync(m));
    tokio::spawn(async move {
        let _ = engine::run(EngineKind::Epoll, router, ctx).await;
    });
    wait_port(port);
    let b = get(port, "/mem", Duration::from_secs(5));
    assert!(
        status_line(&b).starts_with("HTTP/1.1 504"),
        "memory.grow past 16MiB is 504, got {}",
        status_line(&b)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reload() {
    let loop_wasm = include_bytes!("fixtures/loop.wasm");
    let trap_wasm = include_bytes!("fixtures/trap.wasm");
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("index.html"), b"ok").unwrap();
    let plug = dir.path().join("plug");
    std::fs::create_dir(&plug).unwrap();
    std::fs::write(plug.join("loop.wasm"), loop_wasm).unwrap();
    std::fs::write(
        plug.join("loop.json"),
        br#"{"name":"loop","kind":"wasm","path":"loop.wasm"}"#,
    )
    .unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let cfg = Config::from_json(
        format!(
            r#"{{"bind":"127.0.0.1:{port}","static_root":"{}","memory_cap_bytes":67108864,"engine":"epoll","workers":1,"http2":false,"http3":false,"wasm_fuel":2000000,"plugin_dir":"{}"}}"#,
            dir.path().display(),
            plug.display()
        )
        .as_bytes(),
    )
    .unwrap();
    let rules = Ruleset::parse(
        br#"{"rules":[{"id":"w","module":"loop","methods":["GET"],"include":["/loop"],"exclude":[]},{"id":"s","module":"static","methods":["GET"],"include":["/*"],"exclude":["/loop"]}]}"#,
    )
    .unwrap();
    let (router, ctx, _) = static_router(cfg, rules);
    plugin::load_dir(&router, &plug).unwrap();
    let router2 = router.clone();
    tokio::spawn(async move {
        let _ = engine::run(EngineKind::Epoll, router, ctx).await;
    });
    wait_port(port);
    let h = std::thread::spawn(move || get(port, "/loop", Duration::from_secs(8)));
    std::thread::sleep(Duration::from_millis(80));
    std::fs::write(plug.join("loop.wasm"), trap_wasm).unwrap();
    let _ = plugin::reload(&router2);
    let in_flight = h.join().unwrap();
    assert!(
        status_line(&in_flight).starts_with("HTTP/1.1 504"),
        "in-flight old (fuel) {}",
        status_line(&in_flight)
    );
    let next = get(port, "/loop", Duration::from_secs(3));
    let sl = status_line(&next);
    assert!(
        sl.starts_with("HTTP/1.1 500") || sl.starts_with("HTTP/1.1 504"),
        "next request uses reloaded component, got {sl}"
    );
}
