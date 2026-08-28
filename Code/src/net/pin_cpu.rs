//! Pin the current OS thread to a logical CPU. Linux. Failure is non-fatal.
//!
//! The syscall itself comes from the FDS engine (`fds::util::pin_to_core`);
//! this module keeps the Atomos-side convention of `index % ncpu` and the
//! non-fatal `Option` contract shared by both engines.

pub fn ncpu() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .max(1)
}

/// Pin this thread to `index % ncpu`. Returns the chosen CPU, or None.
pub fn pin_to_cpu(index: usize) -> Option<usize> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = index;
        None
    }
    #[cfg(target_os = "linux")]
    {
        let n = ncpu();
        let cpu = index % n;
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
}
