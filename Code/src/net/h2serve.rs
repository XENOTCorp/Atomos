//! HTTP/2 transport with bounded DATA queues and concurrent request/response
//! streaming. Protocol-independent dispatch and file reads live in sibling modules.
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::task::{Context, Poll};

use bytes::Bytes;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::align::LineAtomicU64;
use crate::error::ServeError;
use crate::io::OutBody;
use crate::proto;
use crate::route::Router;

pub struct CountingIo<S> {
    inner: S,
    rx: Arc<LineAtomicU64>,
    tx: Arc<LineAtomicU64>,
}

impl<S> CountingIo<S> {
    pub fn new(inner: S, rx: Arc<LineAtomicU64>, tx: Arc<LineAtomicU64>) -> Self {
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
        let result = Pin::new(&mut self.inner).poll_read(cx, buf);
        if result.is_ready() {
            self.rx
                .v
                .fetch_add((buf.filled().len() - before) as u64, Ordering::Relaxed);
        }
        result
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for CountingIo<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let result = Pin::new(&mut self.inner).poll_write(cx, buf);
        if let Poll::Ready(Ok(len)) = result {
            self.tx.v.fetch_add(len as u64, Ordering::Relaxed);
        }
        result
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> Poll<std::io::Result<usize>> {
        let result = Pin::new(&mut self.inner).poll_write_vectored(cx, bufs);
        if let Poll::Ready(Ok(len)) = result {
            self.tx.v.fetch_add(len as u64, Ordering::Relaxed);
        }
        result
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

pub async fn handle<S>(io: S, peer: SocketAddr, router: Arc<Router>) -> Result<(), ServeError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let Some(_guard) = router.admit_conn(peer) else {
        return Ok(());
    };
    let rx = Arc::new(LineAtomicU64::new(0));
    let tx = Arc::new(LineAtomicU64::new(0));
    let counted = CountingIo::new(io, rx.clone(), tx.clone());
    let mut conn = h2::server::Builder::new()
        .max_concurrent_streams(router.cfg.scheduler.str_max.min(256))
        .max_header_list_size(u32::try_from(router.cfg.max_header_bytes).unwrap_or(u32::MAX))
        .max_frame_size(16 * 1024)
        .max_concurrent_reset_streams(32)
        .max_pending_accept_reset_streams(20)
        .max_local_error_reset_streams(Some(32))
        .handshake(counted)
        .await
        .map_err(h2_err)?;
    router.metrics.h2_conns.v.fetch_add(1, Ordering::Relaxed);
    while let Some(request) = conn.accept().await {
        let (request, mut respond) = request.map_err(h2_err)?;
        let router = router.clone();
        tokio::spawn(async move {
            if let Err(error) = serve_one(request, &mut respond, peer, router.clone()).await {
                if is_reset(&error) {
                    router.metrics.h2_rst.v.fetch_add(1, Ordering::Relaxed);
                }
                tracing::debug!(%error, "h2 stream");
            }
        });
    }
    router
        .metrics
        .h2_wire_in
        .v
        .fetch_add(rx.v.load(Ordering::Relaxed), Ordering::Relaxed);
    router
        .metrics
        .h2_wire_out
        .v
        .fetch_add(tx.v.load(Ordering::Relaxed), Ordering::Relaxed);
    Ok(())
}

async fn serve_one(
    request: http::Request<h2::RecvStream>,
    respond: &mut h2::server::SendResponse<Bytes>,
    peer: SocketAddr,
    router: Arc<Router>,
) -> Result<(), ServeError> {
    let (head, mut recv) = request.into_parts();
    let head_only = head.method == http::Method::HEAD;
    router.metrics.h2_streams.v.fetch_add(1, Ordering::Relaxed);
    router
        .metrics
        .h2_headers_raw
        .v
        .fetch_add(proto::raw_header_bytes(&head), Ordering::Relaxed);
    let (tx, rx) = tokio::sync::mpsc::channel::<Bytes>(16);
    let feed = async {
        let mut body_len = 0usize;
        while let Some(chunk) = recv.data().await {
            let chunk = chunk.map_err(h2_err)?;
            if chunk.len() > router.cfg.max_body_bytes.saturating_sub(body_len) {
                return Err(ServeError::BodyTooLarge);
            }
            body_len += chunk.len();
            recv.flow_control()
                .release_capacity(chunk.len())
                .map_err(h2_err)?;
            if tx.send(chunk).await.is_err() {
                break;
            }
        }
        drop(tx);
        Ok::<_, ServeError>(body_len)
    };
    let response = async {
        let out = proto::stream_dispatch(&router, head, peer, rx).await;
        let end = head_only || !out.status.allows_body() || matches!(out.body, OutBody::Empty);
        let mut send = respond
            .send_response(proto::out_to_http(&out), end)
            .map_err(h2_err)?;
        if end {
            return Ok(());
        }
        match &out.body {
            OutBody::Stream(body) => {
                let mut chunks = body.take();
                while let Some(bytes) = chunks.recv().await {
                    send_data_bounded(&mut send, bytes, false).await?;
                }
                send.send_data(Bytes::new(), true).map_err(h2_err)?;
            }
            OutBody::File(file) => {
                let mut chunks = crate::net::file_body::FileChunks::new(file);
                while let Some(bytes) = chunks.next().await? {
                    send_data_bounded(&mut send, bytes, false).await?;
                }
                send.send_data(Bytes::new(), true).map_err(h2_err)?;
            }
            _ => {
                send_data_bounded(&mut send, out.body.to_bytes().unwrap_or_default(), true).await?
            }
        }
        Ok::<_, ServeError>(())
    };
    // No nested per-stream tasks: both halves make progress in this task and
    // cancellation/error drops the sibling's channels and borrowed resources.
    let (body_len, ()) = tokio::try_join!(feed, response)?;
    router
        .metrics
        .h2_body_in
        .v
        .fetch_add(body_len as u64, Ordering::Relaxed);
    Ok(())
}

async fn send_data_bounded(
    send: &mut h2::SendStream<Bytes>,
    mut data: Bytes,
    end: bool,
) -> Result<(), ServeError> {
    if data.is_empty() {
        send.send_data(data, end).map_err(h2_err)?;
        return Ok(());
    }
    while !data.is_empty() {
        send.reserve_capacity(data.len().min(64 * 1024));
        if send.capacity() == 0 {
            std::future::poll_fn(|cx| send.poll_capacity(cx))
                .await
                .ok_or_else(|| ServeError::Io(std::io::Error::other("h2 stream closed")))?
                .map_err(h2_err)?;
            continue;
        }
        let len = send.capacity().min(data.len());
        let bytes = data.split_to(len);
        send.send_data(bytes, end && data.is_empty())
            .map_err(h2_err)?;
    }
    Ok(())
}

fn is_reset(error: &ServeError) -> bool {
    match error {
        ServeError::Io(error) => error
            .get_ref()
            .and_then(|error| error.downcast_ref::<h2::Error>())
            .is_some_and(|error| error.reason().is_some()),
        _ => false,
    }
}

fn h2_err(error: h2::Error) -> ServeError {
    ServeError::Io(std::io::Error::other(error))
}
