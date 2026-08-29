//! Host overlay (.atomos/host.json).
use std::path::PathBuf;

use serde::Deserialize;

use super::Config;
use crate::error::ServeError;

/// Physical cores from topology, capped by the process CPU set.
/// SMT siblings share a core; pinning one worker per sibling on a
/// dual-core laptop loses the cached GET path to contention.
pub fn physical_cpus() -> u32 {
    let logical = std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(2)
        .max(1);
    let mut cores = std::collections::BTreeSet::new();
    for i in 0..256u32 {
        let path = format!("/sys/devices/system/cpu/cpu{i}/topology/thread_siblings_list");
        let Ok(s) = std::fs::read_to_string(&path) else {
            if i == 0 {
                break;
            }
            continue;
        };
        let id = s
            .split(|c: char| !c.is_ascii_digit())
            .find(|x| !x.is_empty())
            .and_then(|x| x.parse::<u32>().ok());
        if let Some(id) = id {
            cores.insert(id);
        }
    }
    let phys = cores.len() as u32;
    if phys == 0 {
        logical
    } else {
        phys.min(logical)
    }
}

/// `$XDG_RUNTIME_DIR`, else `/run/user/<uid>`, else `/tmp`.
pub fn runtime_dir() -> PathBuf {
    if let Ok(d) = std::env::var("XDG_RUNTIME_DIR") {
        if !d.is_empty() {
            return PathBuf::from(d);
        }
    }
    #[cfg(unix)]
    {
        let uid = unsafe { libc::geteuid() };
        let p = PathBuf::from(format!("/run/user/{uid}"));
        if p.is_dir() {
            return p;
        }
    }
    PathBuf::from("/tmp")
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct HostFacts {
    pub workers: Option<u32>,
    pub cpu_pin: Option<bool>,
    pub cache_bytes: Option<usize>,
    pub cache_entries: Option<usize>,
    pub refuse_ports: Option<Vec<u16>>,
}

impl Config {
    pub fn apply_host_file(&mut self) {
        let path = std::env::var_os("ATOMOS_HOST")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(".atomos/host.json"));
        if path.exists() {
            let _ = self.overlay_host_path(&path);
        }
    }

    pub fn overlay_host_path(&mut self, path: &std::path::Path) -> Result<(), ServeError> {
        let raw = std::fs::read(path)?;
        let h: HostFacts = serde_json::from_slice(&raw)
            .map_err(|e| ServeError::Config(e.to_string().into_boxed_str()))?;
        self.overlay_host(&h);
        self.validate()
    }

    pub fn overlay_host(&mut self, h: &HostFacts) {
        if let Some(w) = h.workers {
            self.workers = w.max(1);
        }
        if let Some(p) = h.cpu_pin {
            self.cpu_pin = p;
        }
        if let Some(b) = h.cache_bytes {
            self.cache_bytes = b.max(1024);
        }
        if let Some(e) = h.cache_entries {
            self.cache_entries = e.max(1);
        }
        if let Some(ref ports) = h.refuse_ports {
            self.refuse_ports = ports.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn physical_cpus_at_least_one() {
        assert!(super::physical_cpus() >= 1);
    }
}
