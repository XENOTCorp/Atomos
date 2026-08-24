//! HTTP/2 prior-knowledge (h2c) and HTTP/2 over TLS. Dispatches through `Router`.
//!
//! The `h2` crate hides HPACK and framing internals, so datapath
//! observability is measured at the app boundary: raw header bytes per
//! request (exact) and wire bytes per connection (a counting IO
//! wrapper). The ratio over repeated identical requests is a real
//! HPACK-compression proxy (static-table hits shrink the wire side).

use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};

use bytes::Bytes;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::error::ServeError;
use crate::io::OutBody;
use crate::proto;
use crate::route::Router;

/// Counts every byte crossing the connection (handshake, HPACK header
/// blocks, frames, bodies) — the wire-side of the compression proxy.
pub struct CountingIo<S> {
    inner: S,
    rx: Arc<AtomicU64>,
    tx: Arc<AtomicU64>,
}

impl<S> CountingIo<S> {
    pub fn new(inner: S, rx: Arc<AtomicU64>, tx: Arc<AtomicU64>) -> Self {
        Self { inner, rx, tx }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for CountingIo<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let res = Pin::new(&mut self.inner).poll_read(cx, buf);
        if res.is_ready() {
            self.rx
                .fetch_add((buf.filled().len() - before) as u64, Ordering::Relaxed);
        }
        res
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for CountingIo<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let res = Pin::new(&mut self.inner).poll_write(cx, buf);
        if let Poll::Ready(Ok(n)) = res {
            self.tx.fetch_add(n as u64, Ordering::Relaxed);
        }
        res
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// Raw (uncompressed) header bytes for the request line + headers.
fn raw_header_bytes(head: &http::request::Parts) -> u64 {
    let mut n = head.method.as_str().len() + head.uri.path().len();
    for (name, value) in head.headers.iter() {
        n += name.as_str().len() + value.as_bytes().len() + 4;
    }
    n as u64
}

pub async fn handle<S>(io: S, peer: SocketAddr, router: Arc<Router>) -> Result<(), ServeError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    // Connection admission (integer scheduler): per-IP + global caps.
    let Some(_conn_guard) = router.admit_conn(peer) else {
        return Ok(());
    };
    let rx = Arc::new(AtomicU64::new(0));
    let tx = Arc::new(AtomicU64::new(0));
    let counted = CountingIo::new(io, rx.clone(), tx.clone());
    let mut conn = h2::server::Builder::new()
        .max_concurrent_streams(256)
        .handshake(counted)
        .await
        .map_err(h2_err)?;
    router.metrics.h2_conns.v.fetch_add(1, Ordering::Relaxed);
    while let Some(req) = conn.accept().await {
        let (req, mut respond) = req.map_err(h2_err)?;
        let router = router.clone();
        tokio::spawn(async move {
            if let Err(e) = serve_one(req, &mut respond, peer, router.clone()).await {
                if e.is_reset() {
                    router.metrics.h2_rst.v.fetch_add(1, Ordering::Relaxed);
                }
                tracing::debug!(%e, "h2 stream");
            }
        });
    }
    router
        .metrics
        .h2_wire_in
        .v
        .fetch_add(rx.load(Ordering::Relaxed), Ordering::Relaxed);
    router
        .metrics
        .h2_wire_out
        .v
        .fetch_add(tx.load(Ordering::Relaxed), Ordering::Relaxed);
    Ok(())
}

async fn serve_one(
    req: http::Request<h2::RecvStream>,
    respond: &mut h2::server::SendResponse<Bytes>,
    peer: SocketAddr,
    router: Arc<Router>,
) -> Result<(), ServeError> {
    let (head, recv) = req.into_parts();
    router.metrics.h2_streams.v.fetch_add(1, Ordering::Relaxed);
    router
        .metrics
        .h2_headers_raw
        .v
        .fetch_add(raw_header_bytes(&head), Ordering::Relaxed);
    // Streaming dispatch: the module sees body chunks as they arrive
    // (mpsc) and may answer with an `OutBody::Stream`. The feed task
    // forwards the request body while the dispatch task runs, so a
    // streaming module processes data as it comes in.
    let (tx, rx) = tokio::sync::mpsc::channel::<Bytes>(16);
    let router2 = router.clone();
    let max_body = router.cfg.max_body_bytes;
    // `head` is moved into the task (no per-request HeaderMap clone;
    // raw_header_bytes above already consumed it).
    let task = tokio::spawn(async move { proto::stream_dispatch(&router2, head, peer, rx).await });
    let mut feed = tokio::spawn(async move {
        let mut body_len: usize = 0;
        let mut recv = recv;
        while let Some(chunk) = recv.data().await {
            let chunk = chunk.map_err(h2_err)?;
            body_len += chunk.len();
            if body_len > max_body {
                return Err(ServeError::BodyTooLarge);
            }
            let _ = recv.flow_control().release_capacity(chunk.len());
            if tx.send(chunk).await.is_err() {
                break; // module closed its side (or gave up)
            }
        }
        Ok(body_len)
    });
    let out = match task.await {
        Ok(out) => out,
        Err(_) => return Err(ServeError::Io(std::io::Error::other("stream task"))),
    };
    let http_res = proto::out_to_http(&out);
    let eos = matches!(out.body, OutBody::Empty);
    let mut send = respond.send_response(http_res, eos).map_err(h2_err)?;
    match out.body {
        OutBody::Stream(s) => {
            let mut out_rx = s.take();
            let mut feed_done = false;
            let mut body_len: usize = 0;
            loop {
                tokio::select! {
                    r = &mut feed, if !feed_done => {
                        feed_done = true;
                        body_len = match r {
                            Ok(Ok(n)) => n,
                            Ok(Err(e)) => {
                                send.send_reset(h2::Reason::REFUSED_STREAM);
                                return Err(e);
                            }
                            Err(_) => 0,
                        };
                    }
                    chunk = out_rx.recv() => {
                        match chunk {
                            Some(c) => send.send_data(c, false).map_err(h2_err)?,
                            None => {
                                // Module finished; wait for the body feed
                                // to complete so the stream is fully
                                // consumed before we end the response.
                                if !feed_done {
                                    body_len = match (&mut feed).await {
                                        Ok(Ok(n)) => n,
                                        Ok(Err(e)) => {
                                            send.send_reset(h2::Reason::REFUSED_STREAM);
                                            return Err(e);
                                        }
                                        Err(_) => 0,
                                    };
                                }
                                send.send_data(Bytes::new(), true).map_err(h2_err)?;
                                break;
                            }
                        }
                    }
                }
            }
            router
                .metrics
                .h2_body_in
                .v
                .fetch_add(body_len as u64, Ordering::Relaxed);
        }
        _ => {
            // Buffered response: drain the body feed, then send once.
            let body_len = match feed.await {
                Ok(Ok(n)) => n,
                Ok(Err(e)) => {
                    send.send_reset(h2::Reason::REFUSED_STREAM);
                    return Err(e);
                }
                Err(_) => 0,
            };
            router
                .metrics
                .h2_body_in
                .v
                .fetch_add(body_len as u64, Ordering::Relaxed);
            if !eos {
                let body = if matches!(out.body, OutBody::File(_)) {
                    crate::proto::materialize_file_body(&out).await
                } else {
                    // Refcount bump, not a copy (the cache already holds
                    // the response body as Bytes).
                    out.body.to_bytes().unwrap_or_default()
                };
                send.send_data(body, true).map_err(h2_err)?;
            }
        }
    }
    Ok(())
}

/// `h2::Error` with an associated `Reason` is a RST_STREAM event
/// (received or sent) — counted as the RST_STREAM rate.
trait ResetTrait {
    fn is_reset(&self) -> bool;
}

impl ResetTrait for ServeError {
    fn is_reset(&self) -> bool {
        match self {
            ServeError::Io(e) => e
                .get_ref()
                .and_then(|r| r.downcast_ref::<h2::Error>())
                .is_some_and(|e| e.reason().is_some()),
            _ => false,
        }
    }
}

fn h2_err(e: h2::Error) -> ServeError {
    ServeError::Io(std::io::Error::other(e))
}
