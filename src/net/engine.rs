//! I/O engine plug. `tokio` (H1/H2/H3) and `epoll` (H1, no spawn).

use std::sync::Arc;

use crate::atom::AtomCtx;
use crate::error::ServeError;
use crate::route::Router;
use crate::serve;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EngineKind {
    /// HTTP/1.1 epoll, one loop per pinned thread, no spawn-per-conn.
    #[default]
    Epoll,
    /// Pinned current-thread tokio. HTTP/2 + HTTP/3 + TLS. Separate process.
    Tokio,
    /// Not compiled. AF_XDP / DPDK. Needs a NIC; not loopback.
    Xdp,
}

impl EngineKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EngineKind::Tokio => "tokio",
            EngineKind::Epoll => "epoll",
            EngineKind::Xdp => "xdp",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "tokio" => EngineKind::Tokio,
            "epoll" => EngineKind::Epoll,
            "xdp" | "af-xdp" => EngineKind::Xdp,
            _ => return None,
        })
    }
}

pub async fn run(
    kind: EngineKind,
    router: Arc<Router>,
    ctx: Arc<AtomCtx>,
) -> Result<(), ServeError> {
    match kind {
        EngineKind::Epoll => {
            // OS thread, not spawn_blocking: tokio runtime shutdown must not wait
            // on the epoll join (tests abort the task; workers die with the process).
            let (tx, rx) = tokio::sync::oneshot::channel();
            std::thread::Builder::new()
                .name("atomos-epoll-join".into())
                .spawn(move || {
                    let _ = tx.send(super::epoll::run(router, ctx));
                })
                .map_err(ServeError::Io)?;
            match rx.await {
                Ok(r) => r,
                Err(_) => Ok(()),
            }
        }
        EngineKind::Tokio => serve::run(router, ctx).await,
        EngineKind::Xdp => Err(ServeError::Config(
            "engine xdp is not linked in this build".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_kind_is_epoll() {
        assert_eq!(EngineKind::default(), EngineKind::Epoll);
    }
}
