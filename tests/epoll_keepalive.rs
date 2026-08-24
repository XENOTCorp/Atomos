//! Keep-alive + wire-cache contract for the FDS-backed epoll engine:
//! one connection must serve many requests (the parse -> cache-hit ->
//! continue loop), the wire cache must short-circuit repeated GETs of
//! the same resource, and a missing path must still 404 on the same
//! connection afterwards.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::Ordering;
use std::time::Duration;

use atomos::config::Config;
use atomos::engine::{self, EngineKind};
use atomos::rules::Ruleset;
use atomos::static_router;

/// True once `resp` holds a complete HTTP/1.1 response (header block
/// terminator + the exact static body, which is sent last).
fn full_response(resp: &[u8]) -> bool {
    resp.windows(4).any(|w| w == b"\r\n\r\n") && resp.ends_with(b"KEEPALIVE-BODY")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn epoll_keepalive_many_requests_and_cache_hit() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("index.html"), b"KEEPALIVE-BODY").unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let cfg = Config::from_json(
        format!(
            r#"{{"bind":"127.0.0.1:{port}","static_root":"{}","memory_cap_bytes":6000000000,"engine":"epoll","workers":1,"http2":false,"http3":false}}"#,
            dir.path().display()
        )
        .as_bytes(),
    )
    .unwrap();
    let rules = Ruleset::parse(
        br#"{"rules":[{"id":"s","module":"static","methods":["GET","HEAD"],"include":["/*"],"exclude":[]}]}"#,
    )
    .unwrap();
    let (router, ctx, _) = static_router(cfg, rules);
    let metrics = router.metrics.clone();
    tokio::spawn(async move {
        let _ = engine::run(EngineKind::Epoll, router, ctx).await;
    });

    let addr = format!("127.0.0.1:{port}");
    let mut s = None;
    for _ in 0..50 {
        if let Ok(c) = TcpStream::connect(&addr) {
            s = Some(c);
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let mut s = s.expect("connect to engine");
    s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();

    // Two keep-alive GETs of the same resource on one connection: the
    // second must short-circuit through the wire cache (never enter
    // dispatch again), and the connection must stay usable for a third,
    // missing, request.
    for _ in 0..2 {
        write!(s, "GET / HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
        let mut resp = Vec::new();
        while !full_response(&resp) {
            let mut buf = [0u8; 4096];
            let n = s.read(&mut buf).unwrap();
            assert_ne!(n, 0, "eof mid-response: {resp:?}");
            resp.extend_from_slice(&buf[..n]);
        }
        let text = String::from_utf8_lossy(&resp);
        assert!(text.starts_with("HTTP/1.1 200"), "{text}");
        assert!(text.contains("KEEPALIVE-BODY"), "{text}");
    }
    // The second GET of a Globally-cached static resource is a
    // WIRE-cache hit: it never re-enters Router::dispatch, so the
    // request counter must still be 1 after two GETs.
    assert_eq!(
        metrics.requests.v.load(Ordering::Relaxed),
        1,
        "second GET must be served from the wire cache"
    );

    // Same connection: a missing path must still produce a 404 page —
    // and that request DOES go through dispatch (counter -> 2).
    write!(s, "GET /nope HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
    let mut resp = Vec::new();
    let mut saw404 = false;
    while !saw404 {
        let mut buf = [0u8; 4096];
        let n = s.read(&mut buf).unwrap();
        assert_ne!(n, 0, "eof mid-404: {resp:?}");
        resp.extend_from_slice(&buf[..n]);
        // "HTTP/1.1 404" is 12 bytes; headers end at the blank line.
        saw404 = resp.windows(12).any(|w| w == b"HTTP/1.1 404")
            && resp.windows(4).any(|w| w == b"\r\n\r\n");
    }
    assert!(String::from_utf8_lossy(&resp).contains("404"));
    assert_eq!(metrics.requests.v.load(Ordering::Relaxed), 2);
}
