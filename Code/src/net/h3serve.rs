//! HTTP/3 over QUIC. One endpoint per pinned worker (`SO_REUSEPORT`).

use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use bytes::{Buf, Bytes};

use crate::atom::AtomCtx;
use crate::error::ServeError;
use crate::io::OutBody;
use crate::proto;
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
    // Connection admission (integer scheduler): per-IP + global caps.
    let Some(_conn_guard) = router.admit_conn(peer) else {
        conn.close(0u32.into(), b"scheduler");
        return Ok(());
    };
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
                    if let Err(e) = serve_one(resolver, peer, router).await {
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

async fn serve_one<C>(
    resolver: h3::server::RequestResolver<C, Bytes>,
    peer: SocketAddr,
    router: Arc<Router>,
) -> Result<(), ServeError>
where
    C: h3::quic::Connection<Bytes>,
    <C as h3::quic::OpenStreams<Bytes>>::BidiStream: h3::quic::BidiStream<Bytes> + Send + 'static,
{
    let (req, stream) = resolver.resolve_request().await.map_err(h3_err)?;
    let (head, _) = req.into_parts();
    router.metrics.h3_streams.v.fetch_add(1, Ordering::Relaxed);
    router
        .metrics
        .h3_headers_raw
        .v
        .fetch_add(proto::raw_header_bytes(&head), Ordering::Relaxed);
    let head_only = head.method == http::Method::HEAD;
    let (mut send_half, mut recv_half) = stream.split();
    let (tx, rx) = tokio::sync::mpsc::channel::<Bytes>(16);
    let feed = async {
        let mut body_len = 0usize;
        while let Some(mut chunk) = recv_half.recv_data().await.map_err(h3_err)? {
            let len = chunk.remaining();
            if len > router.cfg.max_body_bytes.saturating_sub(body_len) {
                return Err(ServeError::BodyTooLarge);
            }
            body_len += len;
            // Buf may be segmented; chunk() need not cover remaining().
            if tx.send(chunk.copy_to_bytes(len)).await.is_err() {
                break;
            }
        }
        drop(tx);
        Ok::<_, ServeError>(body_len)
    };
    let respond = async {
        let out = proto::stream_dispatch(&router, head, peer, rx).await;
        send_half
            .send_response(proto::out_to_http(&out))
            .await
            .map_err(h3_err)?;
        if !head_only && out.status.allows_body() {
            match &out.body {
                OutBody::Stream(body) => {
                    let mut chunks = body.take();
                    while let Some(chunk) = chunks.recv().await {
                        send_half.send_data(chunk).await.map_err(h3_err)?;
                    }
                }
                OutBody::File(file) => {
                    let mut chunks = crate::net::file_body::FileChunks::new(file);
                    while let Some(bytes) = chunks.next().await? {
                        send_half.send_data(bytes).await.map_err(h3_err)?;
                    }
                }
                _ => {
                    let bytes = out.body.to_bytes().unwrap_or_default();
                    if !bytes.is_empty() {
                        send_half.send_data(bytes).await.map_err(h3_err)?;
                    }
                }
            }
        }
        send_half.finish().await.map_err(h3_err)
    };
    // Drain responses while request data is still arriving. Waiting for all
    // input first deadlocks modules with bounded input/output channels.
    // try_join also cancels the sibling on error: no detached dispatch task.
    let (body_len, ()) = tokio::try_join!(feed, respond)?;
    router
        .metrics
        .h3_body_in
        .v
        .fetch_add(body_len as u64, Ordering::Relaxed);
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
