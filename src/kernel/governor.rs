//! RSS/CPU bounds. Hard → 503. Degrade → FLAG_DEGRADED. Criticality C2.

use crate::config::{Config, MemoryMode};

/// RSS is re-read at most this often per worker; memory pressure does
/// not move at request granularity, and a `/proc/self/status` read is
/// ~µs — it must not sit on the hot path (measured: ~60% of CPU on the
/// H2 path before this cache).
const RSS_TTL: std::time::Duration = std::time::Duration::from_millis(100);

thread_local! {
    static RSS_CACHE: std::cell::RefCell<Option<(std::time::Instant, u64)>> =
        const { std::cell::RefCell::new(None) };
}

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

    /// Cached process RSS (thread-local, 100 ms TTL).
    pub fn rss_bytes() -> u64 {
        RSS_CACHE.with(|c| {
            let mut c = c.borrow_mut();
            if let Some((at, v)) = *c {
                if at.elapsed() < RSS_TTL {
                    return v;
                }
            }
            let v = vm_rss_bytes().unwrap_or(0);
            *c = Some((std::time::Instant::now(), v));
            v
        })
    }

    pub fn over_mem(&self) -> bool {
        Self::rss_bytes() > self.cap
    }

    pub fn hard_block(&self) -> bool {
        self.mode == MemoryMode::Hard && self.over_mem()
    }

    /// Process CPU time / (ncpu × wall). Not sampled on the request hot path.
    pub fn cpu_fraction(started: std::time::Instant) -> f32 {
        let ticks = cpu_ticks().unwrap_or(0);
        let hz = clock_ticks_per_sec();
        let wall = started.elapsed().as_secs_f32().max(0.001);
        let n = crate::pin_cpu::ncpu().max(1) as f32;
        (ticks as f32 / hz) / (wall * n)
    }
}

fn clock_ticks_per_sec() -> f32 {
    #[cfg(target_os = "linux")]
    {
        let t = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        if t > 0 {
            return t as f32;
        }
    }
    100.0
}

fn cpu_ticks() -> Option<u64> {
    let s = std::fs::read_to_string("/proc/self/stat").ok()?;
    // pid comm state ppid ... utime(14) stime(15) — comm may contain spaces in parens.
    let rest = s.rsplit(')').next()?;
    let mut it = rest.split_whitespace();
    // fields after comm: state=1 … utime=12, stime=13 in this split
    let utime: u64 = it.nth(11)?.parse().ok()?;
    let stime: u64 = it.next()?.parse().ok()?;
    utime.checked_add(stime)
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
