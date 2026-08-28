//! H2/H3 (tokio) datapath bench against a running `atomos-proto` server.
//!
//! Measures what the `h2`/`h3` crates expose at the app boundary:
//! - req/s and latency (sequential and 64-in-flight multiplexed)
//! - a HOLB proxy: latency of a small GET while a 4 MiB-upload sibling
//!   stream runs concurrently on the SAME connection (true multiplexing
//!   keeps the small stream fast; TCP-level head-of-line would stall it)
//! - an HPACK-compression proxy via the server's `/metrics` counters:
//!   raw header bytes/req (exact) vs wire bytes/req (counting IO
//!   wrapper) for the first request vs after 500 identical ones
//!
//! Usage: `atomos-proto --bind 127.0.0.1:8090 ...` then
//! `cargo run --release --example bench_h23 -- --h2-port 8090 --h3-port 8090`

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use bytes::Bytes;
use h2::client::SendRequest;
use rustls::pki_types::ServerName;
use tokio::net::TcpStream;

fn get(args: &[String], name: &str, default: &str) -> String {
    args.windows(2)
        .find(|w| w[0] == name)
        .map(|w| w[1].clone())
        .unwrap_or_else(|| default.to_string())
}

fn pct(mut v: Vec<u64>, p: f64) -> u64 {
    v.sort_unstable();
    let i = ((v.len() as f64 * p).ceil() as usize).saturating_sub(1).min(v.len().saturating_sub(1));
    v.get(i).copied().unwrap_or(0)
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let h2_port: u16 = get(&args, "--h2-port", "8090").parse().expect("h2 port");
    let h3_port: u16 = get(&args, "--h3-port", "8090").parse().expect("h3 port");
    let count: usize = get(&args, "--count", "2000").parse().expect("count");
    let h2_addr: SocketAddr = format!("127.0.0.1:{h2_port}").parse().expect("h2 addr");
    let h3_addr: SocketAddr = format!("127.0.0.1:{h3_port}").parse().expect("h3 addr");
    println!("# H2/H3 tokio-path bench: h2c {h2_addr} / h3 {h3_addr}, {count} reqs");
    h2_phase(h2_addr, count).await;
    h3_phase(h3_addr, count).await;
}

/// Open an h2c client connection; returns the SendRequest handle.
async fn h2_connect(addr: SocketAddr) -> SendRequest<Bytes> {
    let tcp = TcpStream::connect(addr).await.expect("h2 tcp");
    let (h2, conn) = h2::client::handshake(tcp).await.expect("h2 handshake");
    tokio::spawn(async move {
        let _ = conn.await;
    });
    h2
}

/// One GET returning the response body as bytes.
async fn h2_get_body(h2: &mut SendRequest<Bytes>, uri: &str) -> (u16, Vec<u8>) {
    let req = http::Request::builder().uri(uri).body(()).unwrap();
    let (resp, _send) = h2.send_request(req, true).expect("h2 send");
    let resp = resp.await.expect("h2 resp");
    let status = resp.status().as_u16();
    let mut recv = resp.into_body();
    let mut body = Vec::new();
    while let Some(c) = recv.data().await {
        body.extend_from_slice(&c.expect("h2 data"));
    }
    (status, body)
}

/// One GET, returns (latency ns, status).
async fn h2_get_lat(h2: &mut SendRequest<Bytes>, uri: &str) -> (u64, u16) {
    let t0 = Instant::now();
    let (status, _body) = h2_get_body(h2, uri).await;
    (t0.elapsed().as_nanos() as u64, status)
}

/// Read a numeric counter from the server's Prometheus /metrics.
/// `None` when the server has no metrics route; the HPACK proxy phase
/// then skips instead of aborting the bench.
async fn metric(h2_addr: SocketAddr, name: &str) -> Option<u64> {
    let mut h2 = h2_connect(h2_addr).await;
    let (status, body) = h2_get_body(&mut h2, "/metrics").await;
    drop(h2);
    if status != 200 {
        return None;
    }
    let needle = format!("{name} ");
    String::from_utf8_lossy(&body)
        .lines()
        .find_map(|l| l.strip_prefix(&needle))
        .and_then(|v| v.trim().parse().ok())
}

async fn h2_phase(addr: SocketAddr, count: usize) {
    println!("\n## H2 (h2c)");
    // Sequential latency ladder.
    let mut h2 = h2_connect(addr).await;
    let t0 = Instant::now();
    let mut lat = Vec::with_capacity(count);
    for _ in 0..count {
        let (l, s) = h2_get_lat(&mut h2, "/").await;
        assert_eq!(s, 200);
        lat.push(l);
    }
    let wall = t0.elapsed().as_secs_f64();
    println!(
        "seq: {:.0} req/s: p50 {:.1}us p90 {:.1}us p99 {:.1}us p999 {:.1}us",
        count as f64 / wall,
        pct(lat.clone(), 0.5) as f64 / 1e3,
        pct(lat.clone(), 0.9) as f64 / 1e3,
        pct(lat.clone(), 0.99) as f64 / 1e3,
        pct(lat, 0.999) as f64 / 1e3,
    );
    drop(h2);

    // 64-in-flight multiplexed throughput.
    let mut h2 = h2_connect(addr).await;
    let t0 = Instant::now();
    let mut done = 0usize;
    while done < count {
        let batch = (count - done).min(64);
        let mut futs = Vec::with_capacity(batch);
        for _ in 0..batch {
            let req = http::Request::builder().uri("/").body(()).unwrap();
            let (resp, _send) = h2.send_request(req, true).expect("h2 send");
            futs.push(resp);
        }
        for f in futs {
            let resp = f.await.expect("h2 resp");
            assert_eq!(resp.status().as_u16(), 200);
            let mut recv = resp.into_body();
            while let Some(c) = recv.data().await {
                let _ = c.expect("h2 data");
            }
            done += 1;
        }
    }
    let wall = t0.elapsed().as_secs_f64();
    println!("mux x64: {:.0} req/s (single connection)", count as f64 / wall);
    drop(h2);

    // HOLB proxy: small GET latency with a 256 KiB-upload sibling stream
    // (must stay under the server's max_body_bytes).
    let mut h2 = h2_connect(addr).await;
    let big = http::Request::builder()
        .method("PUT")
        .uri("/")
        .body(())
        .unwrap();
    let (big_resp, mut big_send) = h2.send_request(big, false).expect("big send");
    let chunk = Bytes::from(vec![0xabu8; 64 * 1024]);
    for _ in 0..4 {
        big_send.send_data(chunk.clone(), false).expect("big data");
        tokio::task::yield_now().await;
    }
    // The small GET on the SAME connection while the upload is in flight.
    let mut lat = Vec::with_capacity(200);
    for _ in 0..200 {
        let (l, s) = h2_get_lat(&mut h2, "/").await;
        assert_eq!(s, 200);
        lat.push(l);
    }
    big_send.send_data(Bytes::new(), true).expect("big eos");
    let _ = big_resp.await;
    println!(
        "holb-proxy: small GET p50 {:.1}us p99 {:.1}us while 256 KiB upload in flight (H2 muxes streams)",
        pct(lat.clone(), 0.5) as f64 / 1e3,
        pct(lat, 0.99) as f64 / 1e3,
    );
    drop(h2);

    // HPACK proxy via /metrics wire deltas (counting IO wrapper).
    // Skipped when the server has no metrics route.
    let Some(before) = metric(addr, "atomos_h2_wire_in").await else {
        println!("hpack-proxy: skipped (server does not serve /metrics)");
        return;
    };
    let mut load = h2_connect(addr).await;
    let (_, s) = h2_get_lat(&mut load, "/").await;
    assert_eq!(s, 200);
    drop(load);
    let mid = metric(addr, "atomos_h2_wire_in").await.unwrap();
    let mut load = h2_connect(addr).await;
    for _ in 0..500 {
        let (_, s) = h2_get_lat(&mut load, "/").await;
        assert_eq!(s, 200);
    }
    drop(load);
    let after = metric(addr, "atomos_h2_wire_in").await.unwrap();
    let streams = metric(addr, "atomos_h2_streams").await.unwrap() as i64;
    let raw = metric(addr, "atomos_h2_headers_raw").await.unwrap() as i64;
    let first_wire = (mid - before) as i64;
    let steady_wire = (after - mid) as i64 / 500;
    let raw_per = raw / streams.max(1);
    println!(
        "hpack-proxy: raw headers/req {raw_per}B: wire/req first {first_wire}B, steady {steady_wire}B (static-table hits shrink the wire side)"
    );
}

async fn h3_phase(addr: SocketAddr, count: usize) {
    println!("\n## H3 (QUIC)");
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut client_crypto = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(SkipServerCert))
        .with_no_client_auth();
    client_crypto.alpn_protocols = vec![b"h3".to_vec()];
    let client_crypto = quinn::crypto::rustls::QuicClientConfig::try_from(client_crypto).unwrap();
    let mut endpoint = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).expect("client ep");
    endpoint.set_default_client_config(quinn::ClientConfig::new(Arc::new(client_crypto)));
    let conn = endpoint
        .connect(addr, "localhost")
        .expect("h3 connect")
        .await
        .expect("h3 handshake");
    let (mut driver, mut send) = h3::client::new(h3_quinn::Connection::new(conn))
        .await
        .expect("h3 client");
    tokio::spawn(async move {
        let _ = driver.wait_idle().await;
    });

    async fn get_once(
        send: &mut h3::client::SendRequest<h3_quinn::OpenStreams, Bytes>,
    ) -> u64 {
        use bytes::Buf;
        let t0 = Instant::now();
        let req = http::Request::builder()
            .uri("https://localhost/")
            .body(())
            .unwrap();
        let mut stream = send.send_request(req).await.expect("h3 req");
        stream.finish().await.expect("h3 finish");
        let resp = stream.recv_response().await.expect("h3 resp");
        assert_eq!(resp.status().as_u16(), 200);
        while let Some(mut chunk) = stream.recv_data().await.expect("h3 data") {
            let n = chunk.remaining();
            chunk.advance(n);
        }
        t0.elapsed().as_nanos() as u64
    }

    let t0 = Instant::now();
    let mut lat = Vec::with_capacity(count);
    for _ in 0..count {
        lat.push(get_once(&mut send).await);
    }
    let wall = t0.elapsed().as_secs_f64();
    println!(
        "seq: {:.0} req/s: p50 {:.1}us p90 {:.1}us p99 {:.1}us p999 {:.1}us",
        count as f64 / wall,
        pct(lat.clone(), 0.5) as f64 / 1e3,
        pct(lat.clone(), 0.9) as f64 / 1e3,
        pct(lat.clone(), 0.99) as f64 / 1e3,
        pct(lat, 0.999) as f64 / 1e3,
    );

    // 64-in-flight multiplexed throughput on the same connection.
    let t0 = Instant::now();
    let mut done = 0usize;
    while done < count {
        let batch = (count - done).min(64);
        let mut tasks = Vec::with_capacity(batch);
        for _ in 0..batch {
            let mut s = send.clone();
            tasks.push(tokio::spawn(async move { get_once(&mut s).await }));
        }
        for t in tasks {
            t.await.expect("h3 mux task");
            done += 1;
        }
    }
    let wall = t0.elapsed().as_secs_f64();
    println!("mux x64: {:.0} req/s (single connection)", count as f64 / wall);
}

/// rustls verifier that accepts the server's self-signed cert (bench
/// only; the real client path uses the served CA).
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
