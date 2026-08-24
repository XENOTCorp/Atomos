//! HTTP/3 over QUIC. One endpoint per pinned worker (`SO_REUSEPORT`).

use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use bytes::{Buf, Bytes};

use crate::atom::AtomCtx;
use crate::error::ServeError;
use crate::io::{Out, OutBody};
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
        .fetch_add(raw_header_bytes(&head), Ordering::Relaxed);
    // Split into send/recv halves: the feed runs inline (the recv half
    // is not `Send`); this task streams the response out.
    let (mut send_half, mut recv_half) = stream.split();
    // Streaming dispatch: body chunks flow to the module as they
    // arrive; the module may answer with `OutBody::Stream`. The recv
    // half is not `Send`, so the feed runs inline in the select.
    let (tx, rx) = tokio::sync::mpsc::channel::<Bytes>(16);
    let mut tx = Some(tx);
    let router2 = router.clone();
    let head2 = head.clone();
    let max_body = router.cfg.max_body_bytes;
    let task = tokio::spawn(async move { proto::stream_dispatch(&router2, &head2, peer, rx).await });
    // Run the dispatch task and the body feed concurrently (the recv
    // half is not `Send`, so the feed lives inline here): body chunks
    // reach the module as they arrive, and the task completes when the
    // channel closes (buffered fallback) or promptly (streaming module).
    let mut task = task;
    let mut out: Option<Out> = None;
    let mut body_len: usize = 0;
    loop {
        tokio::select! {
            r = &mut task, if out.is_none() => {
                out = Some(match r {
                    Ok(o) => o,
                    Err(_) => return Err(ServeError::Io(std::io::Error::other("stream task"))),
                });
            }
            chunk = recv_half.recv_data(), if tx.is_some() => {
                match chunk {
                    Ok(Some(mut c)) => {
                        let n = c.remaining();
                        body_len += n;
                        if body_len > max_body {
                            return Err(ServeError::BodyTooLarge);
                        }
                        let b = Bytes::copy_from_slice(c.chunk());
                        c.advance(n);
                        if let Some(t) = &tx {
                            if t.send(b).await.is_err() {
                                tx = None; // module closed early
                            }
                        }
                    }
                    Ok(None) => {
                        tx = None; // drop the sender: closes the channel
                    }
                    Err(e) => return Err(h3_err(e)),
                }
            }
        }
        if out.is_some() && tx.is_none() {
            break;
        }
    }
    let out = out.expect("dispatch task completed");
    router
        .metrics
        .h3_body_in
        .v
        .fetch_add(body_len as u64, Ordering::Relaxed);
    let http_res = proto::out_to_http(&out);
    send_half.send_response(http_res).await.map_err(h3_err)?;
    match out.body {
        OutBody::Stream(s) => {
            // The module produced chunks while the body was feeding;
            // flush them all, then finish.
            let mut out_rx = s.take();
            while let Some(c) = out_rx.recv().await {
                send_half.send_data(c).await.map_err(h3_err)?;
            }
        }
        _ => {
            let body = if matches!(out.body, OutBody::File(_)) {
                crate::proto::materialize_file_body(&out).await
            } else {
                // Refcount bump, not a copy (the cache already holds
                // the response body as Bytes).
                out.body.to_bytes().unwrap_or_default()
            };
            if !body.is_empty() {
                send_half.send_data(body).await.map_err(h3_err)?;
            }
        }
    }
    send_half.finish().await.map_err(h3_err)?;
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
