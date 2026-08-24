//! HTTP/2 prior-knowledge (h2c) and HTTP/2 over TLS. Dispatches through `Router`.

use std::net::SocketAddr;
use std::sync::Arc;

use bytes::{Bytes, BytesMut};
use tokio::io::{AsyncRead, AsyncWrite};

use crate::error::ServeError;
use crate::io::OutBody;
use crate::proto::{self, Parts};
use crate::route::Router;

pub async fn handle<S>(io: S, peer: SocketAddr, router: Arc<Router>) -> Result<(), ServeError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let mut conn = h2::server::Builder::new()
        .max_concurrent_streams(256)
        .handshake(io)
        .await
        .map_err(h2_err)?;
    while let Some(req) = conn.accept().await {
        let (req, mut respond) = req.map_err(h2_err)?;
        let router = router.clone();
        tokio::spawn(async move {
            if let Err(e) = serve_one(req, &mut respond, peer, &router).await {
                tracing::debug!(%e, "h2 stream");
            }
        });
    }
    Ok(())
}

async fn serve_one(
    req: http::Request<h2::RecvStream>,
    respond: &mut h2::server::SendResponse<Bytes>,
    peer: SocketAddr,
    router: &Router,
) -> Result<(), ServeError> {
    let (head, mut recv) = req.into_parts();
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

fn h2_err(e: h2::Error) -> ServeError {
    ServeError::Io(std::io::Error::other(e))
}
