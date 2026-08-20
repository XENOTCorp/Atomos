//! Bounded flag bag passed pre → module → post.
//! Domain: u32 bitset. Criticality C1.

pub const FLAG_LOG: u32 = 1 << 0;
pub const FLAG_METRICS_SKIP: u32 = 1 << 1;
pub const FLAG_NO_POST: u32 = 1 << 2;
pub const FLAG_DEGRADED: u32 = 1 << 3;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FlagSet(pub u32);

impl FlagSet {
    pub const fn empty() -> Self {
        Self(0)
    }

    pub const fn contains(self, bit: u32) -> bool {
        self.0 & bit != 0
    }

    pub fn insert(&mut self, bit: u32) {
        self.0 |= bit;
    }
}
