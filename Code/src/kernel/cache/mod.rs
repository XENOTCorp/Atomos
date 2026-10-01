//! Router-scoped, per-thread response caches. Reads never take a shared lock.
//! Clones share a namespace and invalidation epochs; independent routers do not.
//! Entries are bounded by count and retained wire/body bytes (not process RSS).

mod conditional;
mod storage;
#[cfg(test)]
mod tests;

use std::hash::{Hash, Hasher};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use bytes::Bytes;
use hashbrown::{Equivalent, HashMap};

use crate::align::LineAtomicU64;
use crate::encode::{encode_head, encode_response};
use crate::io::{CacheDirective, In, Method, Out, OutBody};
use crate::status::Status;

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
    generation: u64,
    name: Option<Box<str>>,
    bytes: usize,
    out: Out,
    wire: Arc<Bytes>,
}

type NamedMap = HashMap<Box<str>, u64>;

/// Capture before resolving/invoking a handler. In-flight work cannot refill
/// a generation or named/global epoch that an operator has just invalidated.
pub(crate) struct CacheStamp {
    generation: u64,
    global: u64,
    named: Arc<NamedMap>,
}

/// A cached HTTP/1 response. Ordinary GET/HEAD hits clone only the wire Arc;
/// conditional hits construct a small 304 with the representation validators.
pub enum CachedResponse {
    Wire {
        bytes: Arc<Bytes>,
        status: Status,
        body_len: usize,
    },
    Response(Out),
}

#[derive(Clone)]
pub struct ResponseCache {
    cap: usize,
    cap_bytes: usize,
    pub epoch: Arc<LineAtomicU64>,
    generation: Arc<LineAtomicU64>,
    named: Arc<ArcSwap<NamedMap>>,
}

impl ResponseCache {
    pub fn new(cap: usize, cap_bytes: usize) -> Self {
        Self {
            cap: cap.max(1),
            cap_bytes: cap_bytes.max(1024),
            epoch: Arc::new(LineAtomicU64::new(0)),
            generation: Arc::new(LineAtomicU64::new(0)),
            named: Arc::new(ArcSwap::from_pointee(NamedMap::new())),
        }
    }

    pub fn invalidate(&self) {
        self.epoch.v.fetch_add(1, Ordering::Release);
    }

    /// All entry classes, for rule/module hot-swaps. Ordinary invalidate()
    /// intentionally retains its global-only contract.
    pub fn invalidate_all(&self) {
        self.generation.v.fetch_add(1, Ordering::Release);
    }

    pub(crate) fn stamp(&self) -> CacheStamp {
        CacheStamp {
            generation: self.generation.v.load(Ordering::Acquire),
            global: self.epoch.v.load(Ordering::Acquire),
            named: self.named.load_full(),
        }
    }

    pub fn invalidate_named(&self, id: &str) {
        self.named.rcu(|cur| {
            let mut next = NamedMap::clone(cur);
            let epoch = next.get(id).copied().unwrap_or(0).wrapping_add(1);
            next.insert(id.into(), epoch);
            next
        });
    }

    fn live(&self, entry: &Entry) -> bool {
        if entry.generation != self.generation.v.load(Ordering::Acquire)
            || entry.at.elapsed() > entry.ttl
        {
            return false;
        }
        let epoch = match &entry.name {
            Some(name) => self.named.load().get(name.as_ref()).copied().unwrap_or(0),
            None => self.epoch.v.load(Ordering::Acquire),
        };
        entry.epoch == epoch
    }

    fn with_inner<T>(&self, f: impl FnOnce(&mut storage::Inner) -> T) -> T {
        storage::with_inner(&self.epoch, self.cap, f)
    }

    fn lookup<T>(
        &self,
        method: Method,
        path: &str,
        query: &str,
        f: impl FnOnce(&Entry) -> T,
    ) -> Option<T> {
        let key = Lookup {
            method,
            path,
            query,
        };
        self.with_inner(|inner| {
            let entry = inner.map.get(&key)?;
            self.live(entry).then(|| f(entry))
        })
    }

    pub fn not_modified(headers: &[(&str, &str)], cached: &Out) -> bool {
        conditional::not_modified(headers, cached)
    }

    pub fn request_is_cacheable(req: &In<'_>) -> bool {
        conditional::request_is_cacheable(req)
    }

    pub fn get(&self, method: Method, path: &str, query: &str) -> Option<Out> {
        self.lookup(method, path, query, |entry| entry.out.clone())
    }

    pub fn get_for(&self, req: &In<'_>) -> Option<Out> {
        if !Self::request_is_cacheable(req) {
            return None;
        }
        self.lookup(req.method, req.path, req.query, |entry| {
            if Self::not_modified(&req.headers.pairs, &entry.out) {
                conditional::not_modified_response(&entry.out)
            } else {
                entry.out.clone()
            }
        })
    }

    /// Raw stored wire bytes. Transport code should prefer `get_h1`, which
    /// also enforces request eligibility and conditional-response semantics.
    pub fn get_wire(&self, method: Method, path: &str, query: &str) -> Option<Arc<Bytes>> {
        self.lookup(method, path, query, |entry| entry.wire.clone())
    }

    pub fn get_h1(&self, req: &In<'_>) -> Option<CachedResponse> {
        if !Self::request_is_cacheable(req) {
            return None;
        }
        self.lookup(req.method, req.path, req.query, |entry| {
            if Self::not_modified(&req.headers.pairs, &entry.out) {
                CachedResponse::Response(conditional::not_modified_response(&entry.out))
            } else {
                CachedResponse::Wire {
                    bytes: entry.wire.clone(),
                    status: entry.out.status,
                    body_len: if req.method == Method::Head {
                        0
                    } else {
                        entry.out.body.len()
                    },
                }
            }
        })
    }

    pub fn put(&self, method: Method, path: &str, query: &str, out: &Out) {
        self.put_stamped(method, path, query, out, &self.stamp());
    }

    pub(crate) fn put_stamped(
        &self,
        method: Method,
        path: &str,
        query: &str,
        out: &Out,
        stamp: &CacheStamp,
    ) {
        if stamp.generation != self.generation.v.load(Ordering::Acquire) {
            return;
        }
        if !matches!(method, Method::Get | Method::Head)
            || matches!(out.body, OutBody::Stream(_) | OutBody::File(_))
            || !conditional::response_is_cacheable(out)
            || out.body.len() >= self.cap_bytes
        {
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
        let mut encoded = Vec::with_capacity(512);
        if method == Method::Head {
            encode_head(out, &mut encoded);
        } else {
            encode_response(out, &mut encoded);
        }
        let bytes = encoded.len().saturating_add(out.body.len());
        if bytes > self.cap_bytes {
            return;
        }
        let epoch = match &name {
            Some(name) => stamp.named.get(name.as_ref()).copied().unwrap_or(0),
            None => stamp.global,
        };
        let key = CacheKey {
            method,
            path: path.into(),
            query: query.into(),
        };
        let entry = Entry {
            at: Instant::now(),
            ttl: Duration::from_millis(u64::from(ttl_ms)),
            epoch,
            generation: stamp.generation,
            name,
            bytes,
            out: out.clone(),
            wire: Arc::new(Bytes::from(encoded)),
        };
        self.with_inner(|inner| inner.insert(key, entry, self.cap, self.cap_bytes));
    }
}
