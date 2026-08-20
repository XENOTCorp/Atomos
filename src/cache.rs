//! Bounded response cache. try_lock only. Criticality C1.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use crate::io::{CacheDirective, Method, Out};

struct Entry {
    at: Instant,
    ttl: Duration,
    bytes: usize,
    out: Out,
}

struct Inner {
    map: HashMap<u64, Entry>,
    order: VecDeque<u64>,
    bytes: usize,
}

pub struct ResponseCache {
    inner: Mutex<Inner>,
    cap: usize,
    cap_bytes: usize,
}

impl ResponseCache {
    pub fn new(cap: usize, cap_bytes: usize) -> Self {
        Self {
            inner: Mutex::new(Inner {
                map: HashMap::with_capacity(cap.min(65_536)),
                order: VecDeque::new(),
                bytes: 0,
            }),
            cap: cap.max(1),
            cap_bytes: cap_bytes.max(1024),
        }
    }

    pub fn get(&self, method: Method, path: &str, query: &str) -> Option<Out> {
        let k = key(method, path, query);
        let g = self.inner.try_lock()?;
        let e = g.map.get(&k)?;
        if e.at.elapsed() > e.ttl {
            return None;
        }
        Some(e.out.clone())
    }

    pub fn put(&self, method: Method, path: &str, query: &str, out: &Out) {
        let ttl_ms = match out.cache {
            CacheDirective::No => return,
            CacheDirective::Global { ttl_ms } | CacheDirective::Named { ttl_ms, .. } => ttl_ms,
        };
        if ttl_ms == 0 {
            return;
        }
        let k = key(method, path, query);
        let Some(mut g) = self.inner.try_lock() else {
            return;
        };
        let nbytes = out.body.len();
        while g.map.len() >= self.cap || g.bytes.saturating_add(nbytes) > self.cap_bytes {
            if let Some(old) = g.order.pop_front() {
                if let Some(e) = g.map.remove(&old) {
                    g.bytes = g.bytes.saturating_sub(e.bytes);
                }
            } else {
                break;
            }
        }
        g.map.insert(
            k,
            Entry {
                at: Instant::now(),
                ttl: Duration::from_millis(ttl_ms as u64),
                bytes: nbytes,
                out: out.clone(),
            },
        );
        g.bytes = g.bytes.saturating_add(nbytes);
        g.order.push_back(k);
    }
}

fn key(method: Method, path: &str, query: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    method.as_str().hash(&mut h);
    path.hash(&mut h);
    query.hash(&mut h);
    h.finish()
}
