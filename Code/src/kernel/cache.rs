//! Per-worker (thread-local) response cache. No shared lock. Criticality C1.
//!
//! Keys store method+path+query (collision-safe). A process-wide `epoch`
//! drops Global hits. `invalidate_named` drops only that name. GET never
//! takes a mutex.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::hash::{Hash, Hasher};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use bytes::Bytes;
use hashbrown::Equivalent;
use hashbrown::HashMap;

use crate::align::LineAtomicU64;
use crate::encode::encode_response;
use crate::io::{CacheDirective, Method, Out};

#[derive(Clone, Debug, Eq, PartialEq)]
struct CacheKey {
    method: Method,
    path: Box<str>,
    query: Box<str>,
}

impl Hash for CacheKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        hash_parts(self.method, &self.path, &self.query, state);
    }
}

struct Lookup<'a> {
    method: Method,
    path: &'a str,
    query: &'a str,
}

impl Hash for Lookup<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        hash_parts(self.method, self.path, self.query, state);
    }
}

impl Equivalent<CacheKey> for Lookup<'_> {
    fn equivalent(&self, key: &CacheKey) -> bool {
        self.method == key.method
            && self.path == key.path.as_ref()
            && self.query == key.query.as_ref()
    }
}

fn hash_parts<H: Hasher>(method: Method, path: &str, query: &str, state: &mut H) {
    method.hash(state);
    path.hash(state);
    query.hash(state);
}

struct Entry {
    at: Instant,
    ttl: Duration,
    epoch: u64,
    name: Option<Box<str>>,
    bytes: usize,
    out: Out,
    wire: Arc<Bytes>,
}

struct Inner {
    map: HashMap<CacheKey, Entry>,
    order: VecDeque<CacheKey>,
    bytes: usize,
}

thread_local! {
    static LOCAL: RefCell<Option<Inner>> = const { RefCell::new(None) };
}

type NamedMap = HashMap<Box<str>, u64>;

/// Caps only. The map lives in thread-local storage so workers never share
/// a mutex. Duplicate entries across threads are intended.
#[derive(Clone)]
pub struct ResponseCache {
    cap: usize,
    cap_bytes: usize,
    pub epoch: Arc<LineAtomicU64>,
    named: Arc<ArcSwap<NamedMap>>,
}

impl ResponseCache {
    pub fn new(cap: usize, cap_bytes: usize) -> Self {
        Self {
            cap: cap.max(1),
            cap_bytes: cap_bytes.max(1024),
            epoch: Arc::new(LineAtomicU64::new(0)),
            named: Arc::new(ArcSwap::from_pointee(NamedMap::new())),
        }
    }

    /// Drop all Global cached hits. GET-only; no map lock.
    pub fn invalidate(&self) {
        self.epoch.v.fetch_add(1, Ordering::Release);
    }

    /// Drop hits stored under `CacheDirective::Named { ruleset: id, .. }`.
    pub fn invalidate_named(&self, id: &str) {
        self.named.rcu(|cur| {
            let mut n = NamedMap::clone(cur);
            let v = n.get(id).copied().unwrap_or(0).saturating_add(1);
            n.insert(id.into(), v);
            n
        });
    }

    fn live(&self, e: &Entry) -> bool {
        if e.at.elapsed() > e.ttl {
            return false;
        }
        let cur = match &e.name {
            Some(n) => self.named.load().get(n.as_ref()).copied().unwrap_or(0),
            None => self.epoch.v.load(Ordering::Acquire),
        };
        e.epoch == cur
    }

    fn with_inner<T>(&self, f: impl FnOnce(&mut Inner) -> T) -> T {
        LOCAL.with(|slot| {
            let mut g = slot.borrow_mut();
            if g.is_none() {
                *g = Some(Inner {
                    map: HashMap::with_capacity(self.cap.min(1024)),
                    order: VecDeque::new(),
                    bytes: 0,
                });
            }
            f(g.as_mut().expect("inner"))
        })
    }

    pub fn get(&self, method: Method, path: &str, query: &str) -> Option<Out> {
        let q = Lookup {
            method,
            path,
            query,
        };
        self.with_inner(|inner| {
            let e = inner.map.get(&q)?;
            if !self.live(e) {
                return None;
            }
            Some(e.out.clone())
        })
    }

    /// Cached on-the-wire HTTP/1.1 bytes. Arc clone only.
    pub fn get_wire(&self, method: Method, path: &str, query: &str) -> Option<Arc<Bytes>> {
        let q = Lookup {
            method,
            path,
            query,
        };
        self.with_inner(|inner| {
            let e = inner.map.get(&q)?;
            if !self.live(e) {
                return None;
            }
            Some(e.wire.clone())
        })
    }

    pub fn put(&self, method: Method, path: &str, query: &str, out: &Out) {
        // Streaming bodies are never cacheable: the chunk receiver is
        // single-use and the wire bytes are generated incrementally.
        // File bodies are never cached either: the open fd is a bounded
        // kernel resource held by StaticMod's LRU, not by per-worker
        // entries (each worker would pin its own copy of every fd).
        if matches!(out.body, crate::io::OutBody::Stream(_) | crate::io::OutBody::File(_)) {
            return;
        }
        let (ttl_ms, name) = match &out.cache {
            CacheDirective::No => return,
            CacheDirective::Global { ttl_ms } => (*ttl_ms, None),
            CacheDirective::Named { ruleset, ttl_ms } => (*ttl_ms, Some(ruleset.clone())),
        };
        if ttl_ms == 0 {
            return;
        }
        let k = CacheKey {
            method,
            path: path.into(),
            query: query.into(),
        };
        let mut wire_buf = Vec::with_capacity(512);
        encode_response(out, &mut wire_buf);
        let nbytes = wire_buf.len();
        let wire = Arc::new(Bytes::from(wire_buf));
        let cap = self.cap;
        let cap_bytes = self.cap_bytes;
        let epoch = match &name {
            Some(n) => self.named.load().get(n.as_ref()).copied().unwrap_or(0),
            None => self.epoch.v.load(Ordering::Acquire),
        };
        self.with_inner(|inner| {
            while inner.map.len() >= cap || inner.bytes.saturating_add(nbytes) > cap_bytes {
                if let Some(old) = inner.order.pop_front() {
                    if let Some(e) = inner.map.remove(&old) {
                        inner.bytes = inner.bytes.saturating_sub(e.bytes);
                    }
                } else {
                    break;
                }
            }
            inner.map.insert(
                k.clone(),
                Entry {
                    at: Instant::now(),
                    ttl: Duration::from_millis(ttl_ms as u64),
                    epoch,
                    name,
                    bytes: nbytes,
                    out: out.clone(),
                    wire,
                },
            );
            inner.bytes = inner.bytes.saturating_add(nbytes);
            inner.order.push_back(k);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flags::FlagSet;
    use crate::io::{Out, OutBody};
    use crate::status::Status;

    fn sample() -> Out {
        Out {
            status: Status::OK,
            reason: None,
            headers: vec![],
            body: OutBody::Json(Bytes::from_static(br#"{"ok":true}"#)),
            cache: CacheDirective::Global { ttl_ms: 60_000 },
            flags: FlagSet::empty(),
        }
    }

    fn named(id: &str) -> Out {
        let mut o = sample();
        o.cache = CacheDirective::Named {
            ruleset: id.into(),
            ttl_ms: 60_000,
        };
        o
    }

    #[test]
    fn same_thread_put_get() {
        let c = ResponseCache::new(16, 1 << 20);
        c.put(Method::Get, "/health", "", &sample());
        assert!(c.get(Method::Get, "/health", "").is_some());
        assert!(c.get_wire(Method::Get, "/health", "").is_some());
    }

    #[test]
    fn distinct_paths_are_not_aliases() {
        let c = ResponseCache::new(16, 1 << 20);
        c.put(Method::Get, "/a", "", &sample());
        c.put(Method::Get, "/b", "", &sample());
        assert!(c.get(Method::Get, "/a", "").is_some());
        assert!(c.get(Method::Get, "/b", "").is_some());
        assert!(c.get(Method::Get, "/c", "").is_none());
    }

    #[test]
    fn invalidate_drops_global_not_named() {
        let c = ResponseCache::new(16, 1 << 20);
        c.put(Method::Get, "/g", "", &sample());
        c.put(Method::Get, "/n", "", &named("notes"));
        c.invalidate();
        assert!(c.get_wire(Method::Get, "/g", "").is_none());
        assert!(c.get_wire(Method::Get, "/n", "").is_some());
    }

    #[test]
    fn invalidate_named_drops_only_that_name() {
        let c = ResponseCache::new(16, 1 << 20);
        c.put(Method::Get, "/g", "", &sample());
        c.put(Method::Get, "/n", "", &named("notes"));
        c.invalidate_named("notes");
        assert!(c.get_wire(Method::Get, "/g", "").is_some());
        assert!(c.get_wire(Method::Get, "/n", "").is_none());
    }

    #[test]
    fn epoch_is_one_cache_line() {
        assert_eq!(std::mem::align_of::<crate::align::LineAtomicU64>(), 64);
        assert_eq!(std::mem::size_of::<crate::align::LineAtomicU64>(), 64);
        let c = ResponseCache::new(1, 1024);
        let _ = &c.epoch.v;
    }

    #[test]
    fn other_thread_does_not_see_put() {
        let c = ResponseCache::new(16, 1 << 20);
        c.put(Method::Get, "/only-here", "", &sample());
        assert!(c.get(Method::Get, "/only-here", "").is_some());
        std::thread::scope(|s| {
            s.spawn(|| {
                assert!(
                    c.get(Method::Get, "/only-here", "").is_none(),
                    "per-worker cache must not share maps"
                );
            });
        });
    }
}
