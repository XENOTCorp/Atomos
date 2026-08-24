//! atomos: HTTP kernel in four planes — kernel, net, ops, plugin.
//!
//! - `kernel` — `In`/`Out`, rules, cache, governor
//! - `net` — listen/parse/encode + I/O engines (`EngineKind`)
//! - `ops` — atoms, Unix ctl, supervisor
//! - `plugin` — directory manifests; Wasm slot; `.so` refused
//!
//! Register modules by name, load a disjoint JSON ruleset, call
//! `engine::run(EngineKind::Epoll, router, ctx)` or `epoll::run` (blocking H1).
//! Proto (H2/H3): `serve::run` / `atomos-proto`.
//!
//! Criticality C2. Affine Rust mapping.

#![deny(warnings)]

pub mod kernel;
pub mod net;
pub mod ops;
pub mod plugin;

pub use kernel::align;
pub use kernel::cache;
pub use kernel::config;
pub use kernel::error;
pub use kernel::error_page;
pub use kernel::flags;
pub use kernel::governor;
pub use kernel::io;
pub use kernel::json_out;
pub use kernel::metrics;
pub use kernel::mime;
pub use kernel::module;
pub use kernel::num;
pub use kernel::route;
pub use kernel::rules;
pub use kernel::static_mod;
pub use kernel::status;
pub use net::access_log;
pub use net::encode;
pub use net::engine;
pub use net::epoll;
pub use net::listen;
pub use net::parse;
pub use net::serve;
pub use ops::atom;
pub use ops::control;
pub use ops::control_std;
pub use ops::ctl;
pub use ops::jail;
pub use ops::keyproto;
pub use ops::molecule;
pub use ops::sup;

pub(crate) use net::h2serve;
pub(crate) use net::h3serve;
pub(crate) use net::pin_cpu;
pub(crate) use net::proto;
pub(crate) use net::tls;

use std::sync::Arc;

use arc_swap::ArcSwap;

use crate::align::LineAtomicU8;
use crate::atom::AtomCtx;
use crate::cache::ResponseCache;
use crate::config::Config;
use crate::error_page::ErrorPage;
use crate::governor::Governor;
use crate::metrics::{Metrics, MetricsMod};
use crate::module::{Handler, ModuleMap};
use crate::route::Router;
use crate::rules::Ruleset;
use crate::static_mod::StaticMod;

/// Build a router that only serves static files under `include`/`exclude` in `rules`.
pub fn static_router(cfg: Config, rules: Ruleset) -> (Arc<Router>, Arc<AtomCtx>, Arc<StaticMod>) {
    let errors = ErrorPage::load(&cfg.error_page);
    let st = StaticMod::new(cfg.static_root.clone(), errors.clone());
    let metrics = Arc::new(Metrics::new());
    let mut modules: ModuleMap = hashbrown::HashMap::new();
    modules.insert("static".into(), Handler::Sync(st.clone()));
    modules.insert(
        "metrics".into(),
        Handler::Sync(MetricsMod::new(metrics.clone())),
    );
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
        modules: Arc::new(ArcSwap::from_pointee(modules)),
        pre: None,
        post: None,
        metrics,
        cfg: Arc::new(cfg),
    });
    (router, ctx, st)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::CacheDirective;
    use crate::module::Module;

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
        let a = st.handle(&mk()).unwrap();
        assert_eq!(a.status.as_u16(), 200);
        assert!(matches!(a.cache, CacheDirective::Global { .. }));
        router.cache.put(io::Method::Get, "/x.txt", "", &a);
        assert!(router.cache.get(io::Method::Get, "/x.txt", "").is_some());
    }
}

#[cfg(test)]
mod more {
    use super::*;
    use crate::io::InOwned;
    use crate::module::AsyncModule;

    struct HealthMod;
    impl AsyncModule for HealthMod {
        fn name(&self) -> &'static str {
            "api"
        }
        fn handle<'a>(&'a self, _req: &'a InOwned) -> crate::module::BoxFut<'a> {
            Box::pin(async move {
                Ok(crate::io::Out::json(
                    crate::status::Status::OK,
                    bytes::Bytes::from_static(b"{\"ok\":true}"),
                ))
            })
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
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
        let router = {
            let (router, _, _) = static_router(cfg, rules);
            router.insert(
                "api",
                crate::module::Handler::Async(std::sync::Arc::new(HealthMod)),
            );
            router
        };
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
    }
}
