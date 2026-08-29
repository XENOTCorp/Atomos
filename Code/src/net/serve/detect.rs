//! TCP peek: TLS, HTTP/2 preface, or HTTP/1.1.
use std::sync::Arc;
use tokio_rustls::TlsAcceptor;
use crate::error::ServeError;
use crate::route::Router;
use crate::tls::TlsHold;
use super::H2_PREFACE;
use super::h1::handle_h1;

pub(crate) async fn handle_tcp(
    mut stream: tokio::net::TcpStream,
    peer: std::net::SocketAddr,
    router: Arc<Router>,
    tls: Arc<TlsHold>,
) -> Result<(), ServeError> {
    let _ = stream.set_nodelay(router.cfg.tcp_nodelay);
    let mut peek = [0u8; 24];
    let n = stream.peek(&mut peek).await?;
    if n == 0 {
        return Ok(());
    }
    if peek[0] == 0x16 && (router.cfg.http2 || router.cfg.http3) {
        let set = tls.get()?;
        let acceptor = TlsAcceptor::from(set.tcp.clone());
        let tls_stream = acceptor
            .accept(stream)
            .await
            .map_err(|e| ServeError::Io(std::io::Error::other(e)))?;
        let alpn = tls_stream.get_ref().1.alpn_protocol().map(|p| p.to_vec());
        if router.cfg.http2 && alpn.as_deref() == Some(b"h2") {
            return crate::h2serve::handle(tls_stream, peer, router).await;
        }
        return handle_h1(tls_stream, peer, router).await;
    }
    if router.cfg.http2 && (n >= H2_PREFACE.len() && peek.starts_with(H2_PREFACE)
        || n >= 3 && peek.starts_with(b"PRI"))
    {
        return crate::h2serve::handle(stream, peer, router).await;
    }
    if router.cfg.http2 && !looks_like_h1(&peek[..n]) {
        // Invalid connection preface: GOAWAY PROTOCOL_ERROR then close.
        use tokio::io::AsyncWriteExt;
        const GOAWAY: &[u8] = &[
            0x00, 0x00, 0x08, 0x07, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x01,
        ];
        let _ = stream.write_all(GOAWAY).await;
        let _ = stream.shutdown().await;
        return Ok(());
    }
    handle_h1(stream, peer, router).await
}

fn looks_like_h1(peek: &[u8]) -> bool {
    peek.starts_with(b"GET ")
        || peek.starts_with(b"POST ")
        || peek.starts_with(b"HEAD ")
        || peek.starts_with(b"PUT ")
        || peek.starts_with(b"DELETE ")
        || peek.starts_with(b"PATCH ")
        || peek.starts_with(b"OPTIONS ")
        || peek.starts_with(b"CONNECT ")
        || peek.starts_with(b"TRACE ")
}
