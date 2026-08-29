//! Pin the current OS thread to a logical CPU. Linux. Failure is non-fatal.
//!
//! Affinity comes from FDS (`fds::util::pin_to_core` and
//! `fds::util::physical_cpus`). Worker `i` pins to the first SMT sibling
//! of physical core `i` while the worker count fits on those cores.
//! Extra workers wrap onto logical CPUs. Pinning two workers to a
//! sibling pair (logical 0 then 1 on a 2c/4t host) puts both on one
//! core and leaves the other idle.

pub fn ncpu() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .max(1)
}

/// Logical CPU for worker `index` (physical-core map, then wrap).
pub fn cpu_for_worker(index: usize) -> usize {
    let phys = fds::util::physical_cpus();
    if index < phys.len() {
        phys[index]
    } else {
        index % ncpu()
    }
}

/// Pin this thread to [`cpu_for_worker`]. Returns the chosen CPU, or None.
pub fn pin_to_cpu(index: usize) -> Option<usize> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = index;
        None
    }
    #[cfg(target_os = "linux")]
    {
        let cpu = cpu_for_worker(index);
        match fds::util::pin_to_core(cpu) {
            Ok(()) => Some(cpu),
            Err(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_cpu_zero_does_not_panic() {
        let _ = pin_to_cpu(0);
        assert!(ncpu() >= 1);
    }

    #[test]
    fn workers_map_to_distinct_physical_cores() {
        let phys = fds::util::physical_cpus();
        assert!(!phys.is_empty());
        if phys.len() < 2 {
            return;
        }
        assert_ne!(cpu_for_worker(0), cpu_for_worker(1));
        assert_eq!(cpu_for_worker(0), phys[0]);
        assert_eq!(cpu_for_worker(1), phys[1]);
    }
}
