use super::*;
use crate::flags::FlagSet;
use crate::io::{Body, CacheDirective, HeaderView, InOwned, Method};
use crate::module::{AsyncModule, BoxFut};
use crate::status::Status;
use bytes::Bytes;

struct PublicModule(&'static [u8]);
impl Module for PublicModule {
    fn name(&self) -> &'static str {
        "public"
    }
    fn handle(&self, _: &In<'_>) -> Result<Out, ServeError> {
        let mut out = Out::raw(Status::OK, Bytes::from_static(self.0), "text/plain");
        out.cache = CacheDirective::Global { ttl_ms: 60_000 };
        Ok(out)
    }
}

struct AsyncPublic;
impl AsyncModule for AsyncPublic {
    fn name(&self) -> &'static str {
        "public"
    }
    fn handle<'a>(&'a self, _: &'a InOwned) -> BoxFut<'a> {
        Box::pin(async { PublicModule(b"async").handle(&request(vec![])) })
    }
}

fn router(headers: &str) -> Arc<Router> {
    let cfg = Config::from_json(br#"{"bind":"127.0.0.1:0","workers":1,"memory_cap_bytes":6000000000,"scheduler":{"q_per_ip":1}}"#).unwrap();
    let rules = Ruleset::parse(format!(r#"{{"rules":[{{"id":"public","module":"public","methods":["GET"],"include":["/*"],"headers":{headers}}}]}}"#).as_bytes()).unwrap();
    let (router, _, _) = crate::static_router(cfg, rules);
    router.insert("public", Handler::Sync(Arc::new(PublicModule(b"first"))));
    router
}

fn request<'a>(headers: Vec<(&'a str, &'a str)>) -> In<'a> {
    In {
        method: Method::Get,
        path: "/x",
        query: "",
        headers: HeaderView { pairs: headers },
        body: Body::Empty,
        peer: "127.0.0.1:1".parse().unwrap(),
        flags: FlagSet::empty(),
    }
}

#[test]
fn wire_hits_respect_scheduler_and_memory_limits() {
    let mut router = router("[]");
    assert_eq!(router.dispatch(request(vec![])).status, Status::OK);
    let held = router.admit(request(vec![]).peer).unwrap();
    let Some(CachedResponse::Response(out)) = router.cached_h1(&request(vec![])) else {
        panic!("rejection expected")
    };
    assert_eq!(out.status, Status::SERVICE_UNAVAILABLE);
    drop(held);
    let inner = Arc::get_mut(&mut router).unwrap();
    inner.gov.cap = 1;
    inner.gov.mode = crate::config::MemoryMode::Hard;
    let Some(CachedResponse::Response(out)) = router.cached_h1(&request(vec![])) else {
        panic!("memory rejection expected")
    };
    assert_eq!(out.status, Status::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn sync_and_async_dispatch_admit_exactly_once_and_release_guards() {
    let router = router("[]");
    router.insert("public", Handler::Async(Arc::new(AsyncPublic)));
    for _ in 0..4 {
        let out = router.dispatch_async(request(vec![])).await;
        assert_eq!(out.status, Status::OK);
        assert_eq!(out.body.as_bytes(), b"async");
        assert_eq!(router.sched[0].lock().q_total, 0);
    }
    let held = router.admit(request(vec![]).peer).unwrap();
    assert_eq!(
        router.dispatch_async(request(vec![])).await.status,
        Status::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        router.dispatch(request(vec![])).status,
        Status::SERVICE_UNAVAILABLE
    );
    drop(held);
    assert_eq!(router.sched[0].lock().q_total, 0);
}

#[test]
fn header_rules_cannot_be_bypassed_by_a_cached_response() {
    let router = router(r#"[{"name":"X-Key","exists":true,"on_fail":401}]"#);
    assert_eq!(
        router.dispatch(request(vec![("X-Key", "yes")])).status,
        Status::OK
    );
    assert!(router.cache.get(Method::Get, "/x", "").is_none());
    assert_eq!(
        router.dispatch(request(vec![])).status,
        Status::UNAUTHORIZED
    );
    let router = self::router(r#"[{"name":"X-Forbidden","exists":false}]"#);
    assert_eq!(
        router
            .dispatch(request(vec![("X-Forbidden", "yes")]))
            .status,
        Status::FORBIDDEN
    );
}

#[test]
fn public_handler_replacement_invalidates_old_response() {
    let router = router("[]");
    assert_eq!(router.dispatch(request(vec![])).body.as_bytes(), b"first");
    router.insert("public", Handler::Sync(Arc::new(PublicModule(b"second"))));
    assert_eq!(router.dispatch(request(vec![])).body.as_bytes(), b"second");
}

struct Hook(std::sync::atomic::AtomicUsize);
impl Module for Hook {
    fn name(&self) -> &'static str {
        "hook"
    }
    fn handle(&self, _: &In<'_>) -> Result<Out, ServeError> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(Out::empty(Status::OK))
    }
}

#[tokio::test]
async fn pre_and_post_hooks_are_never_bypassed_by_cached_responses() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    for pre in [true, false] {
        let mut router = router("[]");
        assert_eq!(router.dispatch(request(vec![])).status, Status::OK);
        assert!(router.cached_h1(&request(vec![])).is_some());
        let hook = Arc::new(Hook(AtomicUsize::new(0)));
        let inner = Arc::get_mut(&mut router).unwrap();
        if pre {
            inner.pre = Some(hook.clone());
        } else {
            inner.post = Some(hook.clone());
        }
        assert!(router.cached_h1(&request(vec![])).is_none());
        assert_eq!(router.dispatch(request(vec![])).status, Status::OK);
        assert_eq!(
            router.dispatch_async(request(vec![])).await.status,
            Status::OK
        );
        assert_eq!(hook.0.load(Ordering::Relaxed), 2);
        router.cache.invalidate_all();
        assert_eq!(router.dispatch(request(vec![])).status, Status::OK);
        assert!(router.cache.get(Method::Get, "/x", "").is_none());
    }
}

#[test]
fn body_and_private_requests_are_not_inserted() {
    let router = router("[]");
    let mut req = request(vec![]);
    req.body = Body::Raw(b"body");
    assert_eq!(router.dispatch(req).status, Status::OK);
    assert!(router.cache.get(Method::Get, "/x", "").is_none());
    for header in ["Cookie", "Authorization", "Range"] {
        assert_eq!(
            router.dispatch(request(vec![(header, "x")])).status,
            Status::OK
        );
        assert!(router.cache.get(Method::Get, "/x", "").is_none());
    }
}
