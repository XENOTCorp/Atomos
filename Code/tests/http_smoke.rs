use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use atomos::config::Config;
use atomos::rules::Ruleset;
use atomos::{serve, static_router};

fn http_get(addr: &str, path: &str) -> (u16, String, Vec<u8>) {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    write!(s, "GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").unwrap();
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).unwrap();
    let sep = buf.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = std::str::from_utf8(&buf[..sep]).unwrap();
    let code: u16 = head
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let body = buf[sep + 4..].to_vec();
    (code, head.to_string(), body)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn smoke_on_ephemeral_reported_port() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("index.html"), b"<h1>ATOMOS</h1>").unwrap();
    std::fs::write(dir.path().join("x.txt"), b"hello").unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let cfg = Config::from_json(
        format!(
            r#"{{"bind":"127.0.0.1:{port}","static_root":"{}","memory_cap_bytes":6000000000}}"#,
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
    let stop = ctx.clone();
    tokio::spawn(async move {
        let _ = serve::run(router, stop).await;
    });
    tokio::time::sleep(Duration::from_millis(120)).await;
    let addr = format!("127.0.0.1:{port}");
    let (c, _, body) = http_get(&addr, "/");
    assert_eq!(c, 200);
    assert!(std::str::from_utf8(&body).unwrap().contains("ATOMOS"));
    let (c, head, body) = http_get(&addr, "/x.txt");
    assert_eq!(c, 200);
    assert!(head.to_ascii_lowercase().contains("text/plain"));
    assert_eq!(body, b"hello");
    let (c, _, body) = http_get(&addr, "/nope");
    assert_eq!(c, 404);
    assert!(std::str::from_utf8(&body).unwrap().contains("404"));
}
