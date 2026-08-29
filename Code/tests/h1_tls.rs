//! Epoll H1 TLS 1.3, ALPN http/1.1 only.

use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

use atomos::config::Config;
use atomos::engine::{self, EngineKind};
use atomos::rules::Ruleset;
use atomos::static_router;
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, ClientConnection, HandshakeKind, RootCertStore, StreamOwned};

fn install_ring() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

fn mint_localhost() -> (Vec<u8>, Vec<u8>, CertificateDer<'static>) {
    let ck = rcgen::generate_simple_self_signed(vec!["localhost".into(), "127.0.0.1".into()])
        .unwrap();
    let cert_pem = ck.cert.pem();
    let key_pem = ck.key_pair.serialize_pem();
    let der = CertificateDer::from(ck.cert);
    (cert_pem.into_bytes(), key_pem.into_bytes(), der)
}

async fn boot_tls() -> (u16, tempfile::TempDir, CertificateDer<'static>) {
    install_ring();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("index.html"), b"ok").unwrap();
    let (cert_pem, key_pem, der) = mint_localhost();
    let cert_path = dir.path().join("cert.pem");
    let key_path = dir.path().join("key.pem");
    std::fs::write(&cert_path, &cert_pem).unwrap();
    std::fs::write(&key_path, &key_pem).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let cfg = Config::from_json(
        format!(
            r#"{{"bind":"127.0.0.1:{port}","static_root":"{}","memory_cap_bytes":67108864,"engine":"epoll","workers":1,"http2":false,"http3":false,"tls_cert":"{}","tls_key":"{}"}}"#,
            dir.path().display(),
            cert_path.display(),
            key_path.display()
        )
        .as_bytes(),
    )
    .unwrap();
    assert!(cfg.h1_tls);
    let rules = Ruleset::parse(
        br#"{"rules":[{"id":"s","module":"static","methods":["GET","HEAD"],"include":["/*"],"exclude":[]}]}"#,
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
    (port, dir, der)
}

fn client_cfg(root: CertificateDer<'static>, alpn: &[&[u8]]) -> Arc<ClientConfig> {
    install_ring();
    let mut roots = RootCertStore::empty();
    roots.add(root).unwrap();
    let mut cfg = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    cfg.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
    Arc::new(cfg)
}

fn status_line(buf: &[u8]) -> &str {
    std::str::from_utf8(buf)
        .unwrap_or("")
        .split("\r\n")
        .next()
        .unwrap_or("")
}

fn h1_get(port: u16, cfg: Arc<ClientConfig>, sni: &str) -> io::Result<(HandshakeKind, Vec<u8>)> {
    let sock = TcpStream::connect(("127.0.0.1", port))?;
    sock.set_read_timeout(Some(Duration::from_secs(3)))?;
    sock.set_write_timeout(Some(Duration::from_secs(3)))?;
    let name = ServerName::try_from(sni.to_string()).unwrap();
    let conn = ClientConnection::new(cfg, name).map_err(io::Error::other)?;
    let mut stream = StreamOwned::new(conn, sock);
    stream.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")?;
    let kind = stream.conn.handshake_kind().unwrap_or(HandshakeKind::Full);
    let mut body = Vec::new();
    let _ = stream.read_to_end(&mut body);
    Ok((kind, body))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handshake_ok() {
    let (port, _dir, der) = boot_tls().await;
    let cfg = client_cfg(der, &[b"http/1.1"]);
    let (_kind, body) = h1_get(port, cfg, "localhost").expect("handshake");
    let sl = status_line(&body);
    assert!(sl.starts_with("HTTP/1.1 200"), "{sl:?} {}", String::from_utf8_lossy(&body));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resume_ticket() {
    let (port, _dir, der) = boot_tls().await;
    let cfg = client_cfg(der, &[b"http/1.1"]);
    let (k1, b1) = h1_get(port, cfg.clone(), "localhost").expect("first");
    assert!(status_line(&b1).starts_with("HTTP/1.1 200"), "{k1:?}");
    let (k2, b2) = h1_get(port, cfg, "localhost").expect("second");
    assert!(status_line(&b2).starts_with("HTTP/1.1 200"));
    assert_eq!(k2, HandshakeKind::Resumed, "DidResume kind={k2:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn alpn_h2_only() {
    let (port, _dir, der) = boot_tls().await;
    let cfg = client_cfg(der, &[b"h2"]);
    match h1_get(port, cfg, "localhost") {
        Err(_) => {}
        Ok((_, b)) => {
            assert!(
                b.is_empty() || !status_line(&b).starts_with("HTTP/1.1 200"),
                "h2-only ALPN must close, got {}",
                status_line(&b)
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bad_sni() {
    let (port, _dir, der) = boot_tls().await;
    let cfg = client_cfg(der, &[b"http/1.1"]);
    let r = h1_get(port, cfg, "example.com");
    assert!(r.is_err(), "SNI example.com vs cert localhost must fail");
}
