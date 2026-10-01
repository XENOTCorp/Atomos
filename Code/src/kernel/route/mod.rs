//! Named-handler dispatch. Shared policy lives in `policy`; sync and async
//! entry points differ only in how they invoke the selected handler.
mod policy;
#[cfg(test)]
mod tests;

use std::sync::atomic::Ordering;
use std::sync::Arc;

use arc_swap::ArcSwap;

use crate::cache::{CachedResponse, ResponseCache};
use crate::config::Config;
use crate::error::ServeError;
use crate::error_page::ErrorPage;
use crate::governor::Governor;
use crate::io::{In, Out};
use crate::metrics::Metrics;
use crate::module::{Handler, Module, ModuleMap};
use crate::rules::Ruleset;

pub struct Router {
    pub cfg: Arc<Config>,
    pub rules: Arc<ArcSwap<Ruleset>>,
    pub modules: Arc<ArcSwap<ModuleMap>>,
    pub pre: Option<Arc<dyn Module>>,
    pub post: Option<Arc<dyn Module>>,
    pub cache: ResponseCache,
    pub gov: Governor,
    pub errors: ErrorPage,
    pub metrics: Arc<Metrics>,
    pub sched: Vec<Arc<parking_lot::Mutex<crate::sched::Sched>>>,
}

impl Router {
    fn sched_shard(&self, key: u32) -> &Arc<parking_lot::Mutex<crate::sched::Sched>> {
        &self.sched[(key as usize) % self.sched.len()]
    }

    /// Snapshot query for operators. Dispatch itself does not scan the module
    /// registry: it resolves and invokes only the handler selected by the rule.
    pub fn has_async(&self) -> bool {
        self.modules
            .load()
            .values()
            .any(|handler| matches!(handler, Handler::Async(_)))
    }

    pub fn admit(&self, peer: std::net::SocketAddr) -> Option<crate::sched::ReqGuard> {
        let key = crate::sched::Sched::ip_key(peer);
        let shard = self.sched_shard(key);
        let accepted = shard.lock().admit_request(key) == crate::sched::Admission::Accepted;
        accepted.then(|| crate::sched::ReqGuard {
            sched: shard.clone(),
            key,
        })
    }

    pub fn admit_conn(&self, peer: std::net::SocketAddr) -> Option<crate::sched::ConnGuard> {
        let key = crate::sched::Sched::ip_key(peer);
        let shard = self.sched_shard(key);
        let accepted = shard.lock().admit_conn(key);
        accepted.then(|| crate::sched::ConnGuard {
            sched: shard.clone(),
            key,
        })
    }

    pub fn stream_handler(
        &self,
        method: &http::Method,
        path: &str,
    ) -> Option<Arc<dyn crate::module::AsyncStreamModule>> {
        let rules = self.rules.load();
        let method = crate::io::Method::parse(method.as_str())?;
        let rule = rules.match_method(method, path)?;
        match self.modules.load().get(rule.module.as_ref())? {
            Handler::Stream(handler) => Some(handler.clone()),
            _ => None,
        }
    }

    pub fn module(&self, name: &str) -> Option<Handler> {
        self.modules.load().get(name).cloned()
    }

    pub fn insert(&self, name: impl Into<String>, handler: Handler) {
        let name = name.into();
        self.modules.rcu(|modules| {
            let mut next = ModuleMap::clone(modules);
            next.insert(name.clone(), handler.clone());
            next
        });
        self.cache.invalidate_all();
    }

    pub fn bind_hooks(&mut self) {
        let modules = self.modules.load();
        self.pre = self
            .cfg
            .pre_module
            .as_ref()
            .and_then(|name| match modules.get(name) {
                Some(Handler::Sync(module)) => Some(module.clone()),
                _ => None,
            });
        self.post = self
            .cfg
            .post_module
            .as_ref()
            .and_then(|name| match modules.get(name) {
                Some(Handler::Sync(module)) => Some(module.clone()),
                _ => None,
            });
    }

    /// Allocation-free ordinary wire-cache hits, with the same admission,
    /// memory limits and metrics as normal dispatch. Misses do no policy work;
    /// the ordinary dispatcher will apply it exactly once.
    pub(crate) fn cached_h1(&self, req: &In<'_>) -> Option<CachedResponse> {
        if self.pre.is_some() || self.post.is_some() {
            return None;
        }
        let cached = self.cache.get_h1(req)?;
        let (_guard, _over_mem) = match self.request_guard(req.peer) {
            Ok(admission) => admission,
            Err(out) => return Some(CachedResponse::Response(self.track_bytes(out))),
        };
        self.metrics.hits.v.fetch_add(1, Ordering::Relaxed);
        let bytes = match &cached {
            CachedResponse::Wire { body_len, .. } => *body_len,
            CachedResponse::Response(out) => out.body.len(),
        };
        self.metrics
            .bytes_out
            .v
            .fetch_add(bytes as u64, Ordering::Relaxed);
        Some(cached)
    }

    pub fn dispatch(&self, mut req: In<'_>) -> Out {
        let _guard = match self.begin_request(&mut req) {
            Ok(guard) => guard,
            Err(out) => return self.track_bytes(out),
        };
        let (handler, stamp) = match self.prepare(&mut req) {
            Ok(prepared) => prepared,
            Err(out) => return self.track_bytes(out),
        };
        let result = match handler {
            Handler::Sync(module) => {
                self.metrics.misses.v.fetch_add(1, Ordering::Relaxed);
                module.handle(&req)
            }
            Handler::Async(_) => {
                return self.track_bytes(self.err_out(
                    ServeError::Module("async module requires dispatch_async".into()),
                    "async",
                ))
            }
            Handler::Stream(_) => {
                return self.track_bytes(self.err_out(
                    ServeError::Module("streaming module requires the tokio paths".into()),
                    "stream",
                ))
            }
        };
        self.finish(&req, result, &stamp)
    }

    pub(crate) async fn dispatch_streaming(
        &self,
        head: &http::Request<()>,
        peer: std::net::SocketAddr,
        body: tokio::sync::mpsc::Receiver<bytes::Bytes>,
    ) -> Out {
        let Some(method) = crate::io::Method::parse(head.method().as_str()) else {
            return self.err_out(ServeError::Parse, "method");
        };
        let mut req = In {
            method,
            path: head.uri().path(),
            query: head.uri().query().unwrap_or(""),
            headers: crate::io::HeaderView {
                pairs: head
                    .headers()
                    .iter()
                    .filter_map(|(name, value)| {
                        value.to_str().ok().map(|value| (name.as_str(), value))
                    })
                    .collect(),
            },
            // The body is arriving, not a body-less cacheable GET.
            body: crate::io::Body::Raw(&[]),
            peer,
            flags: crate::flags::FlagSet::empty(),
        };
        let _guard = match self.begin_request(&mut req) {
            Ok(guard) => guard,
            Err(out) => return self.track_bytes(out),
        };
        let (handler, stamp) = match self.prepare(&mut req) {
            Ok(prepared) => prepared,
            Err(out) => return self.track_bytes(out),
        };
        let Handler::Stream(module) = handler else {
            return self.track_bytes(self.err_out(
                ServeError::Module("stream handler changed".into()),
                "stream",
            ));
        };
        self.metrics.misses.v.fetch_add(1, Ordering::Relaxed);
        self.finish(&req, module.handle_streaming(head, body).await, &stamp)
    }

    pub async fn dispatch_async(&self, mut req: In<'_>) -> Out {
        let _guard = match self.begin_request(&mut req) {
            Ok(guard) => guard,
            Err(out) => return self.track_bytes(out),
        };
        let (handler, stamp) = match self.prepare(&mut req) {
            Ok(prepared) => prepared,
            Err(out) => return self.track_bytes(out),
        };
        let result = match handler {
            Handler::Sync(module) => {
                self.metrics.misses.v.fetch_add(1, Ordering::Relaxed);
                module.handle(&req)
            }
            Handler::Async(module) => {
                self.metrics.misses.v.fetch_add(1, Ordering::Relaxed);
                let owned = req.to_owned();
                module.handle(&owned).await
            }
            Handler::Stream(_) => {
                return self.track_bytes(self.err_out(
                    ServeError::Module("streaming module requires the tokio paths".into()),
                    "stream",
                ))
            }
        };
        self.finish(&req, result, &stamp)
    }
}
