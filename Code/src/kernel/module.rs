//! Module trait. Sync first. Async adapter for request paths that await.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::error::ServeError;
use crate::io::{In, InOwned, Out};

pub trait Module: Send + Sync + 'static {
    fn name(&self) -> &'static str;
    /// Run-to-completion. The engine starts a deadline before this
    /// call. Sync `handle` cannot be cancelled mid-function: a 504 is
    /// best-effort after return. Over-budget work is a contract
    /// violation. Wasm maps fuel/epoch/memory traps to 504.
    fn handle(&self, req: &In<'_>) -> Result<Out, ServeError>;
}

pub type BoxFut<'a> = Pin<Box<dyn Future<Output = Result<Out, ServeError>> + Send + 'a>>;

pub trait AsyncModule: Send + Sync + 'static {
    fn name(&self) -> &'static str;
    fn handle<'a>(&'a self, req: &'a InOwned) -> BoxFut<'a>;
}

/// A module that consumes the request body **as it arrives** (chunks
/// through a channel) instead of after the whole body is buffered, and
/// may return an `OutBody::Stream` response produced incrementally.
/// H2/H3 (tokio) only; the sync H1 loop rejects streaming modules.
pub trait AsyncStreamModule: Send + Sync + 'static {
    fn name(&self) -> &'static str;
    fn handle_streaming<'a>(
        &'a self,
        req: &'a http::Request<()>,
        body: tokio::sync::mpsc::Receiver<bytes::Bytes>,
    ) -> BoxFut<'a>;
}

#[derive(Clone)]
pub enum Handler {
    Sync(Arc<dyn Module>),
    Async(Arc<dyn AsyncModule>),
    Stream(Arc<dyn AsyncStreamModule>),
}

pub type ModuleMap = hashbrown::HashMap<String, Handler>;
