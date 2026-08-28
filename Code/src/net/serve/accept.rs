//! Accept loop for the tokio engine.
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::net::TcpListener;
use crate::atom::AtomCtx;
use crate::route::Router;
use crate::tls::TlsHold;
use super::detect::handle_tcp;

pub(crate) async fn accept_loop(
    listener: TcpListener,
    router: Arc<Router>,
    ctx: Arc<AtomCtx>,
    tls: Arc<TlsHold>,
) {
    loop {
        if ctx.stop.v.load(Ordering::Acquire) != 0 {
            break;
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(200)) => {
                if ctx.stop.v.load(Ordering::Acquire) != 0 {
                    break;
                }
            }
            acc = listener.accept() => {
                let Ok((stream, peer)) = acc else { continue };
                let router = router.clone();
                let tls = tls.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_tcp(stream, peer, router, tls).await {
                        tracing::debug!(%e, "conn");
                    }
                });
            }
        }
    }
}
