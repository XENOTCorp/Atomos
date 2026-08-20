//! RSS/CPU bounds. Hard → 503. Degrade → FLAG_DEGRADED. Criticality C2.

use crate::config::{Config, MemoryMode};

pub struct Governor {
    pub cap: u64,
    pub mode: MemoryMode,
}

impl Governor {
    pub fn from_config(c: &Config) -> Self {
        Self {
            cap: c.memory_cap_bytes,
            mode: c.memory_mode,
        }
    }

    pub fn rss_bytes() -> u64 {
        vm_rss_bytes().unwrap_or(0)
    }

    pub fn over_mem(&self) -> bool {
        Self::rss_bytes() > self.cap
    }

    pub fn hard_block(&self) -> bool {
        self.mode == MemoryMode::Hard && self.over_mem()
    }
}

fn vm_rss_bytes() -> Option<u64> {
    let s = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in s.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            let kb: u64 = rest.split_whitespace().next()?.parse().ok()?;
            return kb.checked_mul(1024);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MemoryMode;

    #[test]
    fn tiny_cap_is_over() {
        let g = Governor {
            cap: 1,
            mode: MemoryMode::Hard,
        };
        assert!(g.over_mem());
        assert!(g.hard_block());
    }
}
