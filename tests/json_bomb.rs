use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use atomos::config::Config;
use atomos::governor::Governor;
use atomos::rules::Ruleset;
use atomos::{serve, static_router};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn json_bomb_is_400_and_rss_delta_small() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("index.html"), b"i").unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let cfg = Config::from_json(
        format!(
            r#"{{"bind":"127.0.0.1:{port}","static_root":"{}","memory_cap_bytes":6000000000,"max_json_depth":32}}"#,
            dir.path().display()
        )
        .as_bytes(),
    )
    .unwrap();
    let rules = Ruleset::parse(
        br#"{"rules":[{"id":"s","module":"static","methods":["GET","POST"],"include":["/*"],"exclude":[]}]}"#,
    )
    .unwrap();
    let (router, ctx, _) = static_router(cfg, rules);
    tokio::spawn(async move {
        let _ = serve::run(router, ctx).await;
    });
    tokio::time::sleep(Duration::from_millis(120)).await;
    let rss0 = Governor::rss_bytes();
    let body = vec![b'['; 4096];
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    write!(
        s,
        "POST / HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .unwrap();
    s.write_all(&body).unwrap();
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).unwrap();
    let head = std::str::from_utf8(&buf).unwrap();
    assert!(head.starts_with("HTTP/1.1 400"), "{head}");
    let rss1 = Governor::rss_bytes();
    assert!(
        rss1.saturating_sub(rss0) <= 8 * 1024 * 1024,
        "rss delta {}",
        rss1 - rss0
    );
}
