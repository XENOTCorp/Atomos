//! HTTP/3 over QUIC. One endpoint per pinned worker (`SO_REUSEPORT`).

use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use bytes::{Buf, Bytes, BytesMut};

use crate::atom::AtomCtx;
use crate::error::ServeError;
use crate::io::OutBody;
use crate::proto::{self, Parts};
use crate::route::Router;
use crate::tls::TlsSet;

pub async fn accept_loop(endpoint: quinn::Endpoint, router: Arc<Router>, ctx: Arc<AtomCtx>) {
    loop {
        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_millis(200)) => {
                if ctx.stop.v.load(std::sync::atomic::Ordering::Acquire) != 0 {
                    endpoint.close(0u32.into(), b"stop");
                    break;
                }
            }
            incoming = endpoint.accept() => {
                let Some(incoming) = incoming else { break };
                let router = router.clone();
                tokio::spawn(async move {
                    let Ok(conn) = incoming.await else { return };
                    let peer = conn.remote_address();
                    if let Err(e) = handle_conn(conn, peer, router).await {
                        tracing::debug!(%e, "h3 conn");
                    }
                });
            }
        }
        if ctx.stop.v.load(std::sync::atomic::Ordering::Acquire) != 0 {
            endpoint.close(0u32.into(), b"stop");
            break;
        }
    }
}

async fn handle_conn(
    conn: quinn::Connection,
    peer: SocketAddr,
    router: Arc<Router>,
) -> Result<(), ServeError> {
    router.metrics.h3_conns.v.fetch_add(1, Ordering::Relaxed);
    let mut h3 = h3::server::builder()
        .build(h3_quinn::Connection::new(conn))
        .await
        .map_err(h3_err)?;
    loop {
        match h3.accept().await {
            Ok(Some(resolver)) => {
                let router = router.clone();
                tokio::spawn(async move {
                    if let Err(e) = serve_one(resolver, peer, &router).await {
                        tracing::debug!(%e, "h3 stream");
                    }
                });
            }
            Ok(None) => break,
            Err(e) => {
                tracing::debug!(%e, "h3 accept");
                break;
            }
        }
    }
    Ok(())
}

/// Raw (uncompressed) header bytes for the request line + headers.
fn raw_header_bytes(head: &http::request::Parts) -> u64 {
    let mut n = head.method.as_str().len() + head.uri.path().len();
    for (name, value) in head.headers.iter() {
        n += name.as_str().len() + value.as_bytes().len() + 4;
    }
    n as u64
}

async fn serve_one<C>(
    resolver: h3::server::RequestResolver<C, Bytes>,
    peer: SocketAddr,
    router: &Router,
) -> Result<(), ServeError>
where
    C: h3::quic::Connection<Bytes>,
    <C as h3::quic::OpenStreams<Bytes>>::BidiStream: Send + 'static,
{
    let (req, mut stream) = resolver.resolve_request().await.map_err(h3_err)?;
    let (head, _) = req.into_parts();
    router.metrics.h3_streams.v.fetch_add(1, Ordering::Relaxed);
    router
        .metrics
        .h3_headers_raw
        .v
        .fetch_add(raw_header_bytes(&head), Ordering::Relaxed);
    let mut body = BytesMut::new();
    while let Some(mut chunk) = stream.recv_data().await.map_err(h3_err)? {
        let n = chunk.remaining();
        if body.len().saturating_add(n) > router.cfg.max_body_bytes {
            return Err(ServeError::BodyTooLarge);
        }
        body.extend_from_slice(chunk.chunk());
        chunk.advance(n);
    }
    router
        .metrics
        .h3_body_in
        .v
        .fetch_add(body.len() as u64, Ordering::Relaxed);
    let req = http::Request::from_parts(head, body.freeze());
    let parts: Parts = proto::parts_from_http(req)?;
    let out = proto::dispatch_parts(router, &parts, peer).await;
    let http_res = proto::out_to_http(&out);
    stream.send_response(http_res).await.map_err(h3_err)?;
    if !matches!(out.body, OutBody::Empty) {
        stream
            .send_data(Bytes::copy_from_slice(out.body.as_bytes()))
            .await
            .map_err(h3_err)?;
    }
    stream.finish().await.map_err(h3_err)?;
    Ok(())
}

fn h3_err<E: std::fmt::Display>(e: E) -> ServeError {
    ServeError::Io(std::io::Error::other(e.to_string()))
}

pub fn endpoint_from_std(
    sock: std::net::UdpSocket,
    tls: &TlsSet,
) -> Result<quinn::Endpoint, ServeError> {
    sock.set_nonblocking(true)?;
    quinn::Endpoint::new(
        quinn::EndpointConfig::default(),
        Some(tls.quic.clone()),
        sock,
        Arc::new(quinn::TokioRuntime),
    )
    .map_err(ServeError::from)
}
