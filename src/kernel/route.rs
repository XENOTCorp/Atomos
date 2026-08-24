//! Dispatch: cache → pre → rules → module → post. Criticality C2.

use std::sync::Arc;

use arc_swap::ArcSwap;

use std::sync::atomic::Ordering;

use crate::cache::ResponseCache;
use crate::config::Config;
use crate::error::ServeError;
use crate::error_page::ErrorPage;
use crate::flags::{FlagSet, FLAG_DEGRADED, FLAG_NO_POST};
use crate::governor::Governor;
use crate::io::{CacheDirective, In, Out, OutBody};
use crate::metrics::Metrics;
use crate::module::{Handler, Module, ModuleMap};
use crate::rules::Ruleset;
use crate::status::Status;

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
}

impl Router {
    pub fn has_async(&self) -> bool {
        self.modules
            .load()
            .values()
            .any(|h| matches!(h, crate::module::Handler::Async(_)))
    }

    pub fn module(&self, name: &str) -> Option<Handler> {
        self.modules.load().get(name).cloned()
    }

    /// Hot-swap one named handler. Cache-hit path does not look this up.
    pub fn insert(&self, name: impl Into<String>, h: Handler) {
        let name = name.into();
        self.modules.rcu(|m| {
            let mut n = ModuleMap::clone(m);
            n.insert(name.clone(), h.clone());
            n
        });
    }

    /// Copy `pre_module` / `post_module` from config if those names are registered.
    pub fn bind_hooks(&mut self) {
        let modules = self.modules.load();
        if let Some(name) = &self.cfg.pre_module {
            if let Some(Handler::Sync(m)) = modules.get(name.as_str()) {
                self.pre = Some(m.clone());
            }
        }
        if let Some(name) = &self.cfg.post_module {
            if let Some(Handler::Sync(m)) = modules.get(name.as_str()) {
                self.post = Some(m.clone());
            }
        }
    }
}

impl Router {
    pub fn dispatch(&self, mut req: In<'_>) -> Out {
        self.metrics.requests.v.fetch_add(1, Ordering::Relaxed);
        if self.gov.hard_block() {
            return self.track_bytes(self.err_out(ServeError::Capacity, "resource bound"));
        }
        if self.gov.over_mem() {
            req.flags.insert(FLAG_DEGRADED);
        }
        if let Some(hit) = self.cache.get(req.method, req.path, req.query) {
            self.metrics.hits.v.fetch_add(1, Ordering::Relaxed);
            return self.track_bytes(hit);
        }
        if let Some(pre) = &self.pre {
            match pre.handle(&req) {
                Ok(out) if out.status.as_u16() >= 400 => return self.track_bytes(out),
                Ok(out) => {
                    req.flags = out.flags;
                    if req.flags.contains(FLAG_NO_POST) {
                        // continue
                    }
                }
                Err(e) => return self.track_bytes(self.err_out(e, "pre")),
            }
        }
        let rules = self.rules.load();
        let Some(rule) = rules.match_method(req.method, req.path) else {
            return self.track_bytes(self.err_out(ServeError::NoRule, "no rule"));
        };
        if let Some(hr) = header_fail(rule, &req) {
            return self.track_bytes(self.err_out(hr, "header rule"));
        }
        let modules = self.modules.load();
        let Some(handler) = modules.get(rule.module.as_ref()) else {
            return self.track_bytes(self.err_out(
                ServeError::Module(rule.module.clone()),
                "missing module",
            ));
        };
        let mut out = match handler {
            Handler::Sync(module) => {
                self.metrics.misses.v.fetch_add(1, Ordering::Relaxed);
                match module.handle(&req) {
                    Ok(o) => o,
                    Err(e) => return self.track_bytes(self.err_out(e, "module")),
                }
            }
            Handler::Async(_) => {
                return self.track_bytes(self.err_out(
                    ServeError::Module("async module requires dispatch_async".into()),
                    "async",
                ));
            }
        };
        if let Some(post) = &self.post {
            if !req.flags.contains(FLAG_NO_POST) && !out.flags.contains(FLAG_NO_POST) {
                match post.handle(&req) {
                    Ok(p) => {
                        // Post may rewrite. If it returns a real body, take it.
                        if p.status.as_u16() != 0 {
                            out = merge_post(out, p);
                        }
                    }
                    Err(e) => return self.track_bytes(self.err_out(e, "post")),
                }
            }
        }
        if !matches!(out.cache, CacheDirective::No) {
            self.cache.put(req.method, req.path, req.query, &out);
        }
        self.track_bytes(out)
    }

    pub async fn dispatch_async(&self, mut req: In<'_>) -> Out {
        self.metrics.requests.v.fetch_add(1, Ordering::Relaxed);
        if self.gov.hard_block() {
            return self.track_bytes(self.err_out(ServeError::Capacity, "resource bound"));
        }
        if self.gov.over_mem() {
            req.flags.insert(FLAG_DEGRADED);
        }
        if let Some(hit) = self.cache.get(req.method, req.path, req.query) {
            self.metrics.hits.v.fetch_add(1, Ordering::Relaxed);
            return self.track_bytes(hit);
        }
        if let Some(pre) = &self.pre {
            match pre.handle(&req) {
                Ok(out) if out.status.as_u16() >= 400 => return self.track_bytes(out),
                Ok(out) => {
                    req.flags = out.flags;
                }
                Err(e) => return self.track_bytes(self.err_out(e, "pre")),
            }
        }
        let rules = self.rules.load();
        let Some(rule) = rules.match_method(req.method, req.path) else {
            return self.track_bytes(self.err_out(ServeError::NoRule, "no rule"));
        };
        if let Some(hr) = header_fail(rule, &req) {
            return self.track_bytes(self.err_out(hr, "header rule"));
        }
        let modules = self.modules.load();
        let Some(handler) = modules.get(rule.module.as_ref()) else {
            return self.track_bytes(self.err_out(
                ServeError::Module(rule.module.clone()),
                "missing module",
            ));
        };
        let mut out = match handler {
            Handler::Sync(module) => {
                self.metrics.misses.v.fetch_add(1, Ordering::Relaxed);
                match module.handle(&req) {
                    Ok(o) => o,
                    Err(e) => return self.track_bytes(self.err_out(e, "module")),
                }
            }
            Handler::Async(module) => {
                self.metrics.misses.v.fetch_add(1, Ordering::Relaxed);
                let owned = req.to_owned();
                match module.handle(&owned).await {
                    Ok(o) => o,
                    Err(e) => return self.track_bytes(self.err_out(e, "module")),
                }
            }
        };
        if let Some(post) = &self.post {
            if !req.flags.contains(FLAG_NO_POST) && !out.flags.contains(FLAG_NO_POST) {
                match post.handle(&req) {
                    Ok(p) => {
                        if p.status.as_u16() != 0 {
                            out = merge_post(out, p);
                        }
                    }
                    Err(e) => return self.track_bytes(self.err_out(e, "post")),
                }
            }
        }
        if !matches!(out.cache, CacheDirective::No) {
            self.cache.put(req.method, req.path, req.query, &out);
        }
        self.track_bytes(out)
    }

    fn track_bytes(&self, out: Out) -> Out {
        let n = out.body.len() as u64;
        if n > 0 {
            self.metrics.bytes_out.v.fetch_add(n, Ordering::Relaxed);
        }
        out
    }

    fn err_out(&self, e: ServeError, detail: &str) -> Out {
        let st = Status::from_u16(e.status());
        let body = self.errors.render(st, detail);
        Out {
            status: st,
            reason: None,
            headers: vec![("Content-Type".into(), "text/html; charset=utf-8".into())],
            body: OutBody::Raw(body),
            cache: CacheDirective::No,
            flags: FlagSet::empty(),
        }
    }
}

fn merge_post(mut base: Out, post: Out) -> Out {
    if !post.body.is_empty() {
        base.body = post.body;
    }
    if post.status.as_u16() != 200 || base.status.as_u16() == 200 {
        // post can override status when it set one explicitly via headers? keep post.status if not OK-default.
        if post.status.as_u16() != 200 {
            base.status = post.status;
        }
    }
    base.flags.0 |= post.flags.0;
    base.headers.extend(post.headers);
    base
}

fn header_fail(rule: &crate::rules::Rule, req: &In<'_>) -> Option<ServeError> {
    for h in &rule.headers {
        let got = req.headers.get(&h.name);
        if h.exists == Some(true) && got.is_none() {
            return Some(if h.on_fail == Some(401) {
                ServeError::Unauthorized
            } else {
                ServeError::Forbidden
            });
        }
        if let Some(cidr) = &h.cidr {
            // Minimal: require peer in 127.0.0.0/8 when cidr is that.
            if cidr == "127.0.0.0/8" && !req.peer.ip().is_loopback() {
                return Some(ServeError::Forbidden);
            }
        }
    }
    None
}
