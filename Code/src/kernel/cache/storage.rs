//! Per-thread namespaces and bounded FIFO storage. Reads never take a lock.
use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::{Arc, Weak};

use hashbrown::HashMap;

use super::{CacheKey, Entry};
use crate::align::LineAtomicU64;

pub(super) struct Inner {
    owner: Weak<LineAtomicU64>,
    pub(super) map: HashMap<CacheKey, Entry>,
    order: VecDeque<CacheKey>,
    bytes: usize,
}

impl Inner {
    fn new(owner: &Arc<LineAtomicU64>, cap: usize) -> Self {
        Self {
            owner: Arc::downgrade(owner),
            map: HashMap::with_capacity(cap.min(1024)),
            order: VecDeque::new(),
            bytes: 0,
        }
    }

    pub(super) fn insert(&mut self, key: CacheKey, entry: Entry, cap: usize, cap_bytes: usize) {
        // Do not evict useful entries for an entity that cannot fit at all.
        if entry.bytes > cap_bytes {
            return;
        }
        if let Some(old) = self.map.remove(&key) {
            self.bytes -= old.bytes;
            // A replacement has one FIFO record, not an unbounded trail of
            // stale keys. This work is on insertion, never the read hot path.
            self.order.retain(|queued| queued != &key);
        }
        while self.map.len() >= cap || self.bytes > cap_bytes - entry.bytes {
            let oldest = self
                .order
                .pop_front()
                .expect("nonempty cache has a FIFO record");
            if let Some(old) = self.map.remove(&oldest) {
                self.bytes -= old.bytes;
            }
        }
        self.bytes += entry.bytes;
        self.order.push_back(key.clone());
        self.map.insert(key, entry);
    }

    #[cfg(test)]
    pub(super) fn usage(&self) -> (usize, usize, usize) {
        (self.map.len(), self.order.len(), self.bytes)
    }
}

#[derive(Default)]
struct ThreadCaches {
    // The usual one-router-per-worker case needs only a pointer comparison,
    // not a second hash lookup to find its namespace.
    active: Option<(usize, Inner)>,
    parked: HashMap<usize, Inner>,
}

thread_local! {
    static LOCAL: RefCell<ThreadCaches> = RefCell::new(ThreadCaches::default());
}

pub(super) fn with_inner<T>(
    owner: &Arc<LineAtomicU64>,
    cap: usize,
    f: impl FnOnce(&mut Inner) -> T,
) -> T {
    let id = Arc::as_ptr(owner) as usize;
    LOCAL.with(|slot| {
        let mut caches = slot.borrow_mut();
        if caches.active.as_ref().map(|(key, _)| *key) != Some(id) {
            if let Some((old_id, old)) = caches.active.take() {
                if old.owner.strong_count() != 0 {
                    caches.parked.insert(old_id, old);
                }
            }
            // Weak ownership prevents address-reuse aliasing, and dead
            // routers release their entries on the next namespace switch.
            caches
                .parked
                .retain(|_, inner| inner.owner.strong_count() != 0);
            let inner = caches
                .parked
                .remove(&id)
                .unwrap_or_else(|| Inner::new(owner, cap));
            caches.active = Some((id, inner));
        }
        f(&mut caches.active.as_mut().expect("active cache initialized").1)
    })
}
