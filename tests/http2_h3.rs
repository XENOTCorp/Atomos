//! HTTP/2 prior-knowledge and HTTP/3 against an ephemeral first_app-style server.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use atomos::config::Config;
use atomos::rules::Ruleset;
use atomos::{serve, static_router};
use bytes::Buf;
use rustls::pki_types::ServerName;

fn cfg_and_rules(port: u16, dir: &std::path::Path) -> (Config, Ruleset) {
    let cfg = Config::from_json(
        format!(
            r#"{{"bind":"127.0.0.1:{port}","static_root":"{}","memory_cap_bytes":6000000000,"workers":2,"cpu_pin":true,"engine":"tokio","http2":true,"http3":true}}"#,
            dir.display()
        )
        .as_bytes(),
    )
    .unwrap();
    let rules = Ruleset::parse(
        br#"{"rules":[{"id":"s","module":"static","methods":["GET","HEAD"],"include":["/*"],"exclude":[]}]}"#,
    )
    .unwrap();
    (cfg, rules)
}

async fn boot(dir: &std::path::Path) -> SocketAddr {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let (cfg, rules) = cfg_and_rules(port, dir);
    let (router, ctx, _) = static_router(cfg, rules);
    tokio::spawn(async move {
        let _ = serve::run(router, ctx).await;
    });
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    for _ in 0..50 {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            return addr;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("server did not listen on {addr}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn h2c_get_index() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("index.html"), b"<h1>H2</h1>").unwrap();
    let addr = boot(dir.path()).await;
    let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
    let (h2, conn) = h2::client::handshake(tcp).await.expect("h2 handshake");
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let req = http::Request::builder()
        .uri("http://localhost/")
        .body(())
        .unwrap();
    let mut h2 = h2;
    let (resp, _send) = h2.send_request(req, true).expect("send");
    let resp = resp.await.expect("resp");
    assert_eq!(resp.status(), 200);
    let mut recv = resp.into_body();
    let mut body = Vec::new();
    while let Some(c) = recv.data().await {
        body.extend_from_slice(&c.expect("data"));
    }
    assert!(
        std::str::from_utf8(&body).unwrap().contains("H2"),
        "{}",
        String::from_utf8_lossy(&body)
    );
}

#[derive(Debug)]
struct SkipServerCert;

impl rustls::client::danger::ServerCertVerifier for SkipServerCert {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn h3_get_index() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("index.html"), b"<h1>H3</h1>").unwrap();
    let addr = boot(dir.path()).await;

    let mut client_crypto = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(SkipServerCert))
        .with_no_client_auth();
    client_crypto.alpn_protocols = vec![b"h3".to_vec()];
    let client_crypto = quinn::crypto::rustls::QuicClientConfig::try_from(client_crypto).unwrap();
    let mut endpoint =
        quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).expect("client ep");
    endpoint.set_default_client_config(quinn::ClientConfig::new(Arc::new(client_crypto)));
    let conn = endpoint
        .connect(addr, "localhost")
        .expect("connect")
        .await
        .expect("handshake");
    let (mut driver, mut send) = h3::client::new(h3_quinn::Connection::new(conn))
        .await
        .expect("h3 client");
    tokio::spawn(async move {
        let _ = driver.wait_idle().await;
    });
    let req = http::Request::builder()
        .uri("https://localhost/")
        .body(())
        .unwrap();
    let mut stream = send.send_request(req).await.expect("h3 req");
    stream.finish().await.expect("finish");
    let resp = stream.recv_response().await.expect("h3 resp");
    assert_eq!(resp.status(), 200);
    let mut body = Vec::new();
    while let Some(mut chunk) = stream.recv_data().await.expect("data") {
        let n = chunk.remaining();
        body.extend_from_slice(chunk.chunk());
        chunk.advance(n);
    }
    assert!(
        std::str::from_utf8(&body).unwrap().contains("H3"),
        "{}",
        String::from_utf8_lossy(&body)
    );
}

// --- Streaming (AsyncStreamModule) ---

use atomos::flags::FlagSet;
use atomos::io::{CacheDirective, Out, OutBody, StreamBody};
use atomos::module::{AsyncStreamModule, BoxFut, Handler, ModuleMap};
use atomos::status::Status;

/// Echoes each request-body chunk straight into the response stream —
/// data flows out as it comes in (no whole-body buffering).
struct EchoStream;

impl AsyncStreamModule for EchoStream {
    fn name(&self) -> &'static str {
        "stream"
    }

    fn handle_streaming<'a>(
        &'a self,
        _req: &'a http::Request<()>,
        body: tokio::sync::mpsc::Receiver<bytes::Bytes>,
    ) -> BoxFut<'a> {
        Box::pin(async move {
            let (tx, rx) = tokio::sync::mpsc::channel::<bytes::Bytes>(16);
            tokio::spawn(async move {
                let mut body = body;
                while let Some(c) = body.recv().await {
                    if tx.send(c).await.is_err() {
                        break;
                    }
                }
            });
            Ok(Out {
                status: Status::OK,
                reason: None,
                headers: vec![],
                body: OutBody::Stream(StreamBody(std::sync::Arc::new(
                    parking_lot::Mutex::new(Some(rx)),
                ))),
                cache: CacheDirective::No,
                flags: FlagSet::empty(),
            })
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn h2c_streaming_echo() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("index.html"), b"<h1>H2</h1>").unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let cfg = Config::from_json(
        format!(
            r#"{{"bind":"127.0.0.1:{port}","static_root":"{}","memory_cap_bytes":6000000000,"workers":2,"cpu_pin":false,"engine":"tokio","http2":true,"http3":false}}"#,
            dir.path().display()
        )
        .as_bytes(),
    )
    .unwrap();
    let rules = Ruleset::parse(
        br#"{"rules":[{"id":"s","module":"static","methods":["GET","HEAD"],"include":["/*"],"exclude":["/stream"]},{"id":"st","module":"stream","methods":["POST"],"include":["/stream"],"exclude":[]}]}"#,
    )
    .unwrap();
    let (router, ctx, _) = static_router(cfg, rules);
    // Register the streaming module (hot-swap into the modules map).
    let mut m: ModuleMap = (**router.modules.load()).clone();
    m.insert("stream".into(), Handler::Stream(std::sync::Arc::new(EchoStream)));
    router.modules.store(std::sync::Arc::new(m));
    tokio::spawn(async move {
        let _ = serve::run(router, ctx).await;
    });
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    for _ in 0..50 {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
    let (mut h2, conn) = h2::client::handshake(tcp).await.expect("h2 handshake");
    tokio::spawn(async move {
        let _ = conn.await;
    });
    // POST /stream with a chunked body; each chunk is echoed back.
    let req = http::Request::builder()
        .method("POST")
        .uri("http://localhost/stream")
        .body(())
        .unwrap();
    let (resp, mut send_body) = h2.send_request(req, false).expect("send");
    let chunks: Vec<&[u8]> = vec![b"one-", b"two-", b"three"];
    for c in chunks {
        send_body
            .send_data(bytes::Bytes::from_static(c), false)
            .expect("data");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    send_body
        .send_data(bytes::Bytes::new(), true)
        .expect("eos");
    let resp = resp.await.expect("resp");
    assert_eq!(resp.status(), 200);
    let mut recv = resp.into_body();
    let mut body = Vec::new();
    while let Some(c) = recv.data().await {
        body.extend_from_slice(&c.expect("data"));
    }
    assert_eq!(body, b"one-two-three", "streamed echo must match the request body");
}
