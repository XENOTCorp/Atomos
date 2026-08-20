//! atomos: barebones HTTP kernel.
//!
//! Concurrency model (CC-00): tokio workers accept/parse/write. Modules are
//! synchronous; blocking work belongs in the consumer via spawn_blocking.
//! Ruleset and config sit in `arc-swap`. Shared counters are 64-byte aligned.
//!
//! Complexity: httparse O(headers); JSON depth scan O(body); rules O(R);
//! static open O(path).
//!
//! Extract: copy this crate. Depend with `atomos = { path = "..." }`.
//! Register modules by name, load a disjoint JSON ruleset, call `serve::run`.
//!
//! Criticality C2. Affine Rust mapping.

#![deny(warnings)]

pub mod align;
pub mod atom;
pub mod cache;
pub mod config;
pub mod control;
pub mod error;
pub mod error_page;
pub mod flags;
pub mod governor;
pub mod io;
pub mod listen;
pub mod mime;
pub mod molecule;
pub mod module;
pub mod num;
pub mod parse;
pub mod route;
pub mod rules;
pub mod json_out;
pub mod serve;
pub mod static_mod;
pub mod status;
#[cfg(feature = "tui")]
pub mod tui;

use std::sync::Arc;

use arc_swap::ArcSwap;

use crate::align::LineAtomicU8;
use crate::atom::AtomCtx;
use crate::cache::ResponseCache;
use crate::config::Config;
use crate::error_page::ErrorPage;
use crate::governor::Governor;
use crate::module::{Handler, ModuleMap};
use crate::route::Router;
use crate::rules::Ruleset;
use crate::static_mod::StaticMod;

/// Build a router that only serves static files under `include`/`exclude` in `rules`.
pub fn static_router(cfg: Config, rules: Ruleset) -> (Arc<Router>, Arc<AtomCtx>, Arc<StaticMod>) {
    let errors = ErrorPage::load(&cfg.error_page);
    let st = StaticMod::new(cfg.static_root.clone(), errors.clone());
    let mut modules: ModuleMap = hashbrown::HashMap::new();
    modules.insert("static".into(), Handler::Sync(st.clone()));
    let ctx = Arc::new(AtomCtx {
        signal: Arc::new(LineAtomicU8::new(0)),
        rules: Arc::new(ArcSwap::from_pointee(rules.clone())),
        rules_path: cfg.rules_path.clone(),
        started: std::time::Instant::now(),
        allow_write: true,
        stop: Arc::new(LineAtomicU8::new(0)),
    });
    let router = Arc::new(Router {
        cache: ResponseCache::new(cfg.cache_entries, cfg.cache_bytes),
        gov: Governor::from_config(&cfg),
        errors,
        rules: ctx.rules.clone(),
        modules,
        pre: None,
        post: None,
        cfg: Arc::new(cfg),
    });
    (router, ctx, st)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::CacheDirective;

    #[test]
    fn crate_name_is_extractable() {
        assert_eq!(env!("CARGO_PKG_NAME"), "atomos");
    }

    #[test]
    fn cache_second_hit_skips_module() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("x.txt"), b"hello").unwrap();
        std::fs::write(dir.path().join("index.html"), b"i").unwrap();
        let cfg = Config::from_json(
            format!(
                r#"{{"bind":"127.0.0.1:0","static_root":"{}","memory_cap_bytes":6000000000}}"#,
                dir.path().display()
            )
            .as_bytes(),
        )
        .unwrap();
        let rules = Ruleset::parse(
            br#"{"rules":[{"id":"s","module":"static","methods":["GET"],"include":["/*"],"exclude":[]}]}"#,
        )
        .unwrap();
        let (router, _, st) = static_router(cfg, rules);
        let peer: std::net::SocketAddr = "127.0.0.1:1".parse().unwrap();
        let mk = || io::In {
            method: io::Method::Get,
            path: "/x.txt",
            query: "",
            headers: io::HeaderView { pairs: vec![] },
            body: io::Body::Empty,
            peer,
            flags: flags::FlagSet::empty(),
        };
        let a = router.dispatch(mk());
        assert_eq!(a.status.as_u16(), 200);
        assert!(matches!(a.cache, CacheDirective::Global { .. }));
        let h1 = st.hits.load(std::sync::atomic::Ordering::Relaxed);
        let b = router.dispatch(mk());
        assert_eq!(b.status.as_u16(), 200);
        let h2 = st.hits.load(std::sync::atomic::Ordering::Relaxed);
        assert_eq!(h2, h1, "second GET must be served from cache");
    }

    struct HealthMod;
    impl crate::module::AsyncModule for HealthMod {
        fn name(&self) -> &'static str {
            "api"
        }
        fn handle<'a>(&'a self, req: &'a crate::io::InOwned) -> crate::module::BoxFut<'a> {
            Box::pin(async move {
                if req.path.ends_with("/health") {
                    Ok(crate::io::Out::json(
                        crate::status::Status::OK,
                        bytes::Bytes::from_static(br#"{"ok":true}"#),
                    ))
                } else {
                    Err(crate::error::ServeError::NoRule)
                }
            })
        }
    }

    #[tokio::test]
    async fn static_and_api_health() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("index.html"), b"<p>hi</p>").unwrap();
        let cfg = Config::from_json(
            format!(
                r#"{{"bind":"127.0.0.1:0","static_root":"{}","memory_cap_bytes":6000000000}}"#,
                dir.path().display()
            )
            .as_bytes(),
        )
        .unwrap();
        let rules = Ruleset::parse(
            br#"{"rules":[
              {"id":"s","module":"static","methods":["GET"],"include":["/*"],"exclude":["/api/*"]},
              {"id":"a","module":"api","methods":["GET"],"include":["/api/*"],"exclude":[]}
            ]}"#,
        )
        .unwrap();
        let (mut router, _, _) = static_router(cfg, rules);
        let r = Arc::get_mut(&mut router).expect("unique");
        r.modules.insert(
            "api".into(),
            crate::module::Handler::Async(std::sync::Arc::new(HealthMod)),
        );
        let peer: std::net::SocketAddr = "127.0.0.1:1".parse().unwrap();
        let home = router
            .dispatch_async(io::In {
                method: io::Method::Get,
                path: "/",
                query: "",
                headers: io::HeaderView { pairs: vec![] },
                body: io::Body::Empty,
                peer,
                flags: flags::FlagSet::empty(),
            })
            .await;
        assert_eq!(home.status.as_u16(), 200);
        let health = router
            .dispatch_async(io::In {
                method: io::Method::Get,
                path: "/api/health",
                query: "",
                headers: io::HeaderView { pairs: vec![] },
                body: io::Body::Empty,
                peer,
                flags: flags::FlagSet::empty(),
            })
            .await;
        assert_eq!(health.status.as_u16(), 200);
        assert_eq!(health.body.as_bytes(), br#"{"ok":true}"#);
    }
}
