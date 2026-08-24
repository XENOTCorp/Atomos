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

use bytes::{Bytes, BytesMut};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::error::ServeError;
use crate::io::OutBody;
use crate::proto::{self, Parts};
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
            if let Err(e) = serve_one(req, &mut respond, peer, &router).await {
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
    router: &Router,
) -> Result<(), ServeError> {
    let (head, mut recv) = req.into_parts();
    router.metrics.h2_streams.v.fetch_add(1, Ordering::Relaxed);
    router
        .metrics
        .h2_headers_raw
        .v
        .fetch_add(raw_header_bytes(&head), Ordering::Relaxed);
    let mut body = BytesMut::new();
    while let Some(chunk) = recv.data().await {
        let chunk = chunk.map_err(h2_err)?;
        if body.len().saturating_add(chunk.len()) > router.cfg.max_body_bytes {
            respond.send_reset(h2::Reason::REFUSED_STREAM);
            return Err(ServeError::BodyTooLarge);
        }
        let _ = recv.flow_control().release_capacity(chunk.len());
        body.extend_from_slice(&chunk);
    }
    router
        .metrics
        .h2_body_in
        .v
        .fetch_add(body.len() as u64, Ordering::Relaxed);
    let req = http::Request::from_parts(head, body.freeze());
    let parts: Parts = proto::parts_from_http(req)?;
    let out = proto::dispatch_parts(router, &parts, peer).await;
    let http_res = proto::out_to_http(&out);
    let eos = matches!(out.body, OutBody::Empty);
    let mut send = respond.send_response(http_res, eos).map_err(h2_err)?;
    if !eos {
        send.send_data(Bytes::copy_from_slice(out.body.as_bytes()), true)
            .map_err(h2_err)?;
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
