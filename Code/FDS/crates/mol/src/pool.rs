//! Fully initialized, heap-backed object pools with explicit raw-slot safety.
use crate::ring::MpmcRing;
use core::cell::UnsafeCell;
use core::marker::PhantomData;

pub struct Pool<T, const N: usize> {
    slots: Box<[UnsafeCell<T>; N]>,
    free: MpmcRing<usize, N>,
}

// SAFETY: safe access is mediated by one exclusive guard per free-list slot.
// Raw index operations are unsafe and require the same ownership discipline.
unsafe impl<T: Send, const N: usize> Sync for Pool<T, N> {}

impl<T, const N: usize> Pool<T, N> {
    pub fn new() -> Self
    where
        T: Default,
    {
        Self::from_fn(|_| T::default())
    }

    /// Initialize on the heap before making any slot available. Collection
    /// also drops already-created values if the initializer panics.
    pub fn from_fn(init: impl FnMut(usize) -> T) -> Self {
        let free = MpmcRing::new();
        let slots = (0..N)
            .map(init)
            .map(UnsafeCell::new)
            .collect::<Vec<_>>()
            .into_boxed_slice()
            .try_into()
            .unwrap_or_else(|_| unreachable!("initializer produces exactly N values"));
        let pool = Self { slots, free };
        for index in 0..N {
            assert!(pool.free.try_push(index).is_ok());
        }
        pool
    }

    /// Exclusive access permits replacing initialized values without leaking
    /// the old value or racing an outstanding guard.
    pub fn initialize(&mut self, index: usize, value: T) {
        *self.slots[index].get_mut() = value;
    }

    pub fn try_alloc(&self) -> Option<PoolGuard<'_, T, N>> {
        let index = self.free.try_pop()?;
        Some(PoolGuard {
            pool: self,
            index,
            marker: PhantomData,
        })
    }

    /// Manual ownership for completion-driven users. Prefer try_alloc().
    pub fn try_alloc_index(&self) -> Option<usize> {
        self.free.try_pop()
    }

    /// # Safety
    /// The caller must own this allocated index, hold no live references or
    /// guard to its value, and release it exactly once.
    pub unsafe fn release_index(&self, index: usize) {
        assert!(index < N, "pool index out of bounds");
        self.release(index);
    }

    /// # Safety
    /// The caller must own the allocated slot exclusively. No other reference
    /// or kernel operation may access the value during the returned borrow.
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn get_mut(&self, index: usize) -> &mut T {
        assert!(index < N, "pool index out of bounds");
        unsafe { &mut *self.slots[index].get() }
    }

    fn release(&self, index: usize) {
        let mut index = index;
        loop {
            match self.free.try_push(index) {
                Ok(()) => return,
                Err(value) => index = value,
            }
        }
    }

    /// Approximate during concurrent allocations; exact when quiescent.
    pub fn in_use(&self) -> usize {
        N - self.free.len().min(N)
    }
}

impl<T: Default, const N: usize> Default for Pool<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

pub struct PoolGuard<'a, T, const N: usize> {
    pool: &'a Pool<T, N>,
    index: usize,
    // Without this marker, the guard would incorrectly be Sync for Send-only
    // values such as Cell, because Pool itself is intentionally Sync for them.
    marker: PhantomData<T>,
}

impl<T, const N: usize> PoolGuard<'_, T, N> {
    pub fn index(&self) -> usize {
        self.index
    }
}

impl<T, const N: usize> core::ops::Deref for PoolGuard<'_, T, N> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: initialization is complete and this guard exclusively owns its slot.
        unsafe { &*self.pool.slots[self.index].get() }
    }
}

impl<T, const N: usize> core::ops::DerefMut for PoolGuard<'_, T, N> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: no other safe accessor can obtain the slot until this guard drops.
        unsafe { &mut *self.pool.slots[self.index].get() }
    }
}

impl<T, const N: usize> Drop for PoolGuard<'_, T, N> {
    fn drop(&mut self) {
        self.pool.release(self.index);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::cell::Cell;
    use static_assertions::{assert_impl_all, assert_not_impl_any};
    assert_impl_all!(Pool<Cell<u32>, 4>: Send, Sync);
    assert_impl_all!(PoolGuard<'static, Cell<u32>, 4>: Send);
    assert_not_impl_any!(PoolGuard<'static, Cell<u32>, 4>: Sync);

    #[test]
    fn default_pool_is_immediately_safe_to_read() {
        let pool = Pool::<String, 4>::new();
        assert_eq!(&*pool.try_alloc().unwrap(), "");
    }

    #[test]
    fn pool_alloc_return_cycle() {
        let pool = Pool::<u64, 4>::from_fn(|index| index as u64);
        let first = pool.try_alloc().unwrap();
        assert_eq!(*first, first.index() as u64);
        drop(first);
        let guards: Vec<_> = (0..4).map(|_| pool.try_alloc().unwrap()).collect();
        assert!(pool.try_alloc().is_none());
        assert_eq!(pool.in_use(), 4);
        drop(guards);
        assert_eq!(pool.in_use(), 0);
    }

    #[test]
    fn concurrent_guards_exclusively_access_send_only_values() {
        let pool = Pool::<Cell<u32>, 4>::new();
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    for _ in 0..1_000 {
                        loop {
                            if let Some(slot) = pool.try_alloc() {
                                slot.set(slot.get() + 1);
                                break;
                            }
                            std::thread::yield_now();
                        }
                    }
                });
            }
        });
        assert_eq!(pool.in_use(), 0);
        let guards: Vec<_> = (0..4).map(|_| pool.try_alloc().unwrap()).collect();
        assert_eq!(guards.iter().map(|slot| slot.get()).sum::<u32>(), 8_000);
    }

    #[test]
    fn initializer_panic_drops_completed_values() {
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        struct Count(Arc<AtomicUsize>);
        impl Drop for Count {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }
        let count = Arc::new(AtomicUsize::new(0));
        let result = std::panic::catch_unwind(|| {
            Pool::<Count, 4>::from_fn(|index| {
                assert!(index < 2, "initializer failed");
                Count(count.clone())
            })
        });
        assert!(result.is_err());
        assert_eq!(count.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn initialized_values_and_replacements_are_dropped() {
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        struct Count(Arc<AtomicUsize>);
        impl Drop for Count {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }
        let count = Arc::new(AtomicUsize::new(0));
        let mut pool = Pool::<Count, 4>::from_fn(|_| Count(count.clone()));
        pool.initialize(0, Count(count.clone()));
        assert_eq!(count.load(Ordering::Relaxed), 1);
        drop(pool);
        assert_eq!(count.load(Ordering::Relaxed), 5);
    }
}
