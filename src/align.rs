//! Cache-line isolation for shared atomics. Prevents false sharing.
//! Domain: process-wide signals and per-worker counters. Criticality C2.

use std::sync::atomic::{AtomicU8, AtomicU64};

pub const STATE_OFF: u8 = 0;
pub const STATE_ON: u8 = 1;
pub const STATE_RESTARTING: u8 = 2;

#[repr(C, align(64))]
pub struct LineAtomicU8 {
    pub v: AtomicU8,
    _pad: [u8; 63],
}

impl LineAtomicU8 {
    pub const fn new(x: u8) -> Self {
        Self {
            v: AtomicU8::new(x),
            _pad: [0; 63],
        }
    }
}

#[repr(C, align(64))]
pub struct LineAtomicU64 {
    pub v: AtomicU64,
    _pad: [u8; 56],
}

impl LineAtomicU64 {
    pub const fn new(x: u64) -> Self {
        Self {
            v: AtomicU64::new(x),
            _pad: [0; 56],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_atomic_u8_is_one_cache_line() {
        assert_eq!(std::mem::align_of::<LineAtomicU8>(), 64);
        assert_eq!(std::mem::size_of::<LineAtomicU8>(), 64);
    }

    #[test]
    fn line_atomic_u64_is_one_cache_line() {
        assert_eq!(std::mem::align_of::<LineAtomicU64>(), 64);
        assert_eq!(std::mem::size_of::<LineAtomicU64>(), 64);
    }
}
