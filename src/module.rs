//! Module trait. Sync first. Async adapter for request paths that await.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::error::ServeError;
use crate::io::{In, InOwned, Out};

pub trait Module: Send + Sync + 'static {
    fn name(&self) -> &'static str;
    fn handle(&self, req: &In<'_>) -> Result<Out, ServeError>;
}

pub type BoxFut<'a> = Pin<Box<dyn Future<Output = Result<Out, ServeError>> + Send + 'a>>;

pub trait AsyncModule: Send + Sync + 'static {
    fn name(&self) -> &'static str;
    fn handle<'a>(&'a self, req: &'a InOwned) -> BoxFut<'a>;
}

pub enum Handler {
    Sync(Arc<dyn Module>),
    Async(Arc<dyn AsyncModule>),
}

pub type ModuleMap = hashbrown::HashMap<String, Handler>;
