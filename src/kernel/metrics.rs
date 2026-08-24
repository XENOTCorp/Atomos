//! Process-wide request counters. Cache-hit path never looks up modules.
//! Domain: Relaxed atomics; scrape is a pure snapshot.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use bytes::Bytes;

use crate::align::LineAtomicU64;
use crate::error::ServeError;
use crate::io::{CacheDirective, In, Out, OutBody};
use crate::module::Module;
use crate::num::u64_to_slice;
use crate::status::Status;

pub struct Metrics {
    pub requests: LineAtomicU64,
    pub hits: LineAtomicU64,
    pub misses: LineAtomicU64,
    pub bytes_out: LineAtomicU64,
    // Tokio H2/H3 datapath observability (the `h2`/`h3` crates hide
    // HPACK/QPACK internals, so these are measured at the app boundary:
    // raw header bytes per request are exact; wire bytes per connection
    // come from a counting IO wrapper and make a compression proxy).
    pub h2_conns: LineAtomicU64,
    pub h2_streams: LineAtomicU64,
    pub h2_headers_raw: LineAtomicU64,
    pub h2_body_in: LineAtomicU64,
    pub h2_rst: LineAtomicU64,
    pub h2_wire_in: LineAtomicU64,
    pub h2_wire_out: LineAtomicU64,
    pub h3_conns: LineAtomicU64,
    pub h3_streams: LineAtomicU64,
    pub h3_headers_raw: LineAtomicU64,
    pub h3_body_in: LineAtomicU64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub requests: u64,
    pub hits: u64,
    pub misses: u64,
    pub bytes_out: u64,
    pub h2_conns: u64,
    pub h2_streams: u64,
    pub h2_headers_raw: u64,
    pub h2_body_in: u64,
    pub h2_rst: u64,
    pub h2_wire_in: u64,
    pub h2_wire_out: u64,
    pub h3_conns: u64,
    pub h3_streams: u64,
    pub h3_headers_raw: u64,
    pub h3_body_in: u64,
}

impl Metrics {
    pub fn new() -> Self {
        Self {
            requests: LineAtomicU64::new(0),
            hits: LineAtomicU64::new(0),
            misses: LineAtomicU64::new(0),
            bytes_out: LineAtomicU64::new(0),
            h2_conns: LineAtomicU64::new(0),
            h2_streams: LineAtomicU64::new(0),
            h2_headers_raw: LineAtomicU64::new(0),
            h2_body_in: LineAtomicU64::new(0),
            h2_rst: LineAtomicU64::new(0),
            h2_wire_in: LineAtomicU64::new(0),
            h2_wire_out: LineAtomicU64::new(0),
            h3_conns: LineAtomicU64::new(0),
            h3_streams: LineAtomicU64::new(0),
            h3_headers_raw: LineAtomicU64::new(0),
            h3_body_in: LineAtomicU64::new(0),
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            requests: self.requests.v.load(Ordering::Relaxed),
            hits: self.hits.v.load(Ordering::Relaxed),
            misses: self.misses.v.load(Ordering::Relaxed),
            bytes_out: self.bytes_out.v.load(Ordering::Relaxed),
            h2_conns: self.h2_conns.v.load(Ordering::Relaxed),
            h2_streams: self.h2_streams.v.load(Ordering::Relaxed),
            h2_headers_raw: self.h2_headers_raw.v.load(Ordering::Relaxed),
            h2_body_in: self.h2_body_in.v.load(Ordering::Relaxed),
            h2_rst: self.h2_rst.v.load(Ordering::Relaxed),
            h2_wire_in: self.h2_wire_in.v.load(Ordering::Relaxed),
            h2_wire_out: self.h2_wire_out.v.load(Ordering::Relaxed),
            h3_conns: self.h3_conns.v.load(Ordering::Relaxed),
            h3_streams: self.h3_streams.v.load(Ordering::Relaxed),
            h3_headers_raw: self.h3_headers_raw.v.load(Ordering::Relaxed),
            h3_body_in: self.h3_body_in.v.load(Ordering::Relaxed),
        }
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Module `metrics`: Prometheus text from a shared `Metrics`.
pub struct MetricsMod {
    metrics: Arc<Metrics>,
}

impl MetricsMod {
    pub fn new(metrics: Arc<Metrics>) -> Arc<Self> {
        Arc::new(Self { metrics })
    }
}

impl Module for MetricsMod {
    fn name(&self) -> &'static str {
        "metrics"
    }

    fn handle(&self, _req: &In<'_>) -> Result<Out, ServeError> {
        let s = self.metrics.snapshot();
        let mut body = Vec::with_capacity(256);
        let mut nbuf = [0u8; 20];
        push_metric(&mut body, b"atomos_requests ", s.requests, &mut nbuf);
        push_metric(&mut body, b"atomos_cache_hits ", s.hits, &mut nbuf);
        push_metric(&mut body, b"atomos_cache_misses ", s.misses, &mut nbuf);
        push_metric(&mut body, b"atomos_bytes_out ", s.bytes_out, &mut nbuf);
        push_metric(&mut body, b"atomos_h2_conns ", s.h2_conns, &mut nbuf);
        push_metric(&mut body, b"atomos_h2_streams ", s.h2_streams, &mut nbuf);
        push_metric(&mut body, b"atomos_h2_headers_raw ", s.h2_headers_raw, &mut nbuf);
        push_metric(&mut body, b"atomos_h2_body_in ", s.h2_body_in, &mut nbuf);
        push_metric(&mut body, b"atomos_h2_rst ", s.h2_rst, &mut nbuf);
        push_metric(&mut body, b"atomos_h2_wire_in ", s.h2_wire_in, &mut nbuf);
        push_metric(&mut body, b"atomos_h2_wire_out ", s.h2_wire_out, &mut nbuf);
        push_metric(&mut body, b"atomos_h3_conns ", s.h3_conns, &mut nbuf);
        push_metric(&mut body, b"atomos_h3_streams ", s.h3_streams, &mut nbuf);
        push_metric(&mut body, b"atomos_h3_headers_raw ", s.h3_headers_raw, &mut nbuf);
        push_metric(&mut body, b"atomos_h3_body_in ", s.h3_body_in, &mut nbuf);
        Ok(Out {
            status: Status::OK,
            reason: None,
            headers: vec![("Content-Type".into(), "text/plain; charset=utf-8".into())],
            body: OutBody::Raw(Bytes::from(body)),
            cache: CacheDirective::No,
            flags: crate::flags::FlagSet::empty(),
        })
    }
}

fn push_metric(dst: &mut Vec<u8>, name: &[u8], n: u64, nbuf: &mut [u8; 20]) {
    dst.extend_from_slice(name);
    let k = u64_to_slice(n, nbuf);
    dst.extend_from_slice(&nbuf[..k]);
    dst.push(b'\n');
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::ResponseCache;
    use crate::config::Config;
    use crate::error_page::ErrorPage;
    use crate::flags::FlagSet;
    use crate::governor::Governor;
    use crate::io::{Body, HeaderView, Method};
    use crate::module::{Handler, ModuleMap};
    use crate::route::Router;
    use crate::rules::Ruleset;
    use arc_swap::ArcSwap;
    use std::sync::atomic::AtomicU64;

    #[test]
    fn snapshot_is_pure() {
        let m = Metrics::new();
        m.requests.v.fetch_add(1, Ordering::Relaxed);
        let a = m.snapshot();
        let b = m.snapshot();
        assert_eq!(a.requests, b.requests);
        assert_eq!(a, b);
    }

    struct CountMod {
        n: AtomicU64,
    }

    impl Module for CountMod {
        fn name(&self) -> &'static str {
            "count"
        }

        fn handle(&self, _req: &In<'_>) -> Result<Out, ServeError> {
            self.n.fetch_add(1, Ordering::Relaxed);
            Ok(Out {
                status: Status::OK,
                reason: None,
                headers: vec![("Content-Type".into(), "text/plain".into())],
                body: OutBody::Raw(Bytes::from_static(b"ok")),
                cache: CacheDirective::Global { ttl_ms: 60_000 },
                flags: FlagSet::empty(),
            })
        }
    }

    #[test]
    fn dispatch_hit_miss_counters() {
        let cfg = Config::from_json(
            br#"{"bind":"127.0.0.1:0","static_root":".","memory_cap_bytes":6000000000}"#,
        )
        .unwrap();
        let rules = Ruleset::parse(
            br#"{"rules":[{"id":"c","module":"count","methods":["GET"],"include":["/c"],"exclude":[]}]}"#,
        )
        .unwrap();
        let metrics = Arc::new(Metrics::new());
        let count = Arc::new(CountMod {
            n: AtomicU64::new(0),
        });
        let mut modules: ModuleMap = hashbrown::HashMap::new();
        modules.insert("count".into(), Handler::Sync(count.clone()));
        let router = Router {
            cache: ResponseCache::new(64, 1 << 20),
            gov: Governor::from_config(&cfg),
            errors: ErrorPage::builtin(),
            rules: Arc::new(ArcSwap::from_pointee(rules)),
            modules: Arc::new(ArcSwap::from_pointee(modules)),
            pre: None,
            post: None,
            metrics: metrics.clone(),
            cfg: Arc::new(cfg),
            sched: Arc::new(parking_lot::Mutex::new(crate::sched::Sched::new(
                crate::sched::RuleMode::default(),
                crate::sched::Weights::default(),
                crate::sched::Limits::default(),
            ))),
        };
        let peer: std::net::SocketAddr = "127.0.0.1:1".parse().unwrap();
        let mk = || In {
            method: Method::Get,
            path: "/c",
            query: "",
            headers: HeaderView { pairs: vec![] },
            body: Body::Empty,
            peer,
            flags: FlagSet::empty(),
        };
        let a = router.dispatch(mk());
        assert_eq!(a.status.as_u16(), 200);
        let b = router.dispatch(mk());
        assert_eq!(b.status.as_u16(), 200);
        let s = metrics.snapshot();
        assert_eq!(s.misses, 1);
        assert!(s.hits >= 1);
        assert_eq!(count.n.load(Ordering::Relaxed), 1);
        assert_eq!(s.requests, 2);
    }
}
