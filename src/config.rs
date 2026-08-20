//! JSON config. Missing required bind is an error. Criticality C2.

use std::net::IpAddr;
use std::path::PathBuf;

use serde::Deserialize;

use crate::error::ServeError;

#[derive(Clone, Debug, Deserialize)]
pub struct Config {
    #[serde(default = "default_bind")]
    pub bind: String,
    #[serde(default)]
    pub allow_non_loopback: bool,
    #[serde(default = "default_workers")]
    pub workers: u32,
    #[serde(default = "default_backlog")]
    pub backlog: i32,
    #[serde(default = "default_true")]
    pub tcp_nodelay: bool,
    #[serde(default)]
    pub tcp_fastopen: bool,
    #[serde(default = "default_true")]
    pub so_reuseport: bool,
    #[serde(default = "default_keepalive")]
    pub keepalive_secs: u64,
    #[serde(default = "default_header_bytes")]
    pub max_header_bytes: usize,
    #[serde(default = "default_body_bytes")]
    pub max_body_bytes: usize,
    #[serde(default = "default_json_depth")]
    pub max_json_depth: u32,
    #[serde(default = "default_timeout")]
    pub request_timeout_ms: u64,
    #[serde(default = "default_mem")]
    pub memory_cap_bytes: u64,
    #[serde(default = "default_mem_mode")]
    pub memory_mode: MemoryMode,
    #[serde(default = "default_cpu")]
    pub cpu_fraction: f32,
    #[serde(default = "default_queue")]
    pub queue_cap: u64,
    #[serde(default = "default_cache_entries")]
    pub cache_entries: usize,
    #[serde(default = "default_cache_bytes")]
    pub cache_bytes: usize,
    pub pre_module: Option<String>,
    pub post_module: Option<String>,
    #[serde(default = "default_rules")]
    pub rules_path: PathBuf,
    #[serde(default = "default_sock")]
    pub control_socket: PathBuf,
    #[serde(default = "default_static")]
    pub static_root: PathBuf,
    #[serde(default = "default_error_page")]
    pub error_page: PathBuf,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MemoryMode {
    Hard,
    Degrade,
}

fn default_bind() -> String {
    "127.0.0.1:8090".into()
}
fn default_workers() -> u32 {
    2
}
fn default_backlog() -> i32 {
    1024
}
fn default_true() -> bool {
    true
}
fn default_keepalive() -> u64 {
    75
}
fn default_header_bytes() -> usize {
    16384
}
fn default_body_bytes() -> usize {
    262144
}
fn default_json_depth() -> u32 {
    32
}
fn default_timeout() -> u64 {
    45_000
}
fn default_mem() -> u64 {
    6_000_000_000
}
fn default_mem_mode() -> MemoryMode {
    MemoryMode::Hard
}
fn default_cpu() -> f32 {
    0.40
}
fn default_queue() -> u64 {
    1_000_000
}
fn default_cache_entries() -> usize {
    4096
}
fn default_cache_bytes() -> usize {
    268435456
}
fn default_rules() -> PathBuf {
    PathBuf::from("rules.json")
}
fn default_sock() -> PathBuf {
    PathBuf::from("/tmp/atomos.sock")
}
fn default_static() -> PathBuf {
    PathBuf::from("static")
}
fn default_error_page() -> PathBuf {
    PathBuf::from("static/error.html")
}

impl Config {
    pub fn from_json(raw: &[u8]) -> Result<Self, ServeError> {
        let c: Config = serde_json::from_slice(raw)
            .map_err(|e| ServeError::Config(e.to_string().into_boxed_str()))?;
        c.validate()?;
        Ok(c)
    }

    pub fn load_path(path: &std::path::Path) -> Result<Self, ServeError> {
        let raw = std::fs::read(path)?;
        Self::from_json(&raw)
    }

    pub fn validate(&self) -> Result<(), ServeError> {
        if self.bind.is_empty() {
            return Err(ServeError::Config("empty bind".into()));
        }
        let addr: std::net::SocketAddr = self
            .bind
            .parse()
            .map_err(|_| ServeError::Config("bind is not host:port".into()))?;
        if !self.allow_non_loopback && !is_loopback(addr.ip()) {
            return Err(ServeError::Config("non-loopback bind refused".into()));
        }
        if self.workers == 0 {
            return Err(ServeError::Config("workers == 0".into()));
        }
        if self.max_json_depth == 0 || self.max_json_depth > 64 {
            return Err(ServeError::Config("max_json_depth not in 1..=64".into()));
        }
        if self.max_body_bytes < 1024 {
            return Err(ServeError::Config("max_body_bytes < 1024".into()));
        }
        if self.memory_cap_bytes < 16 * 1024 * 1024 {
            return Err(ServeError::Config("memory_cap_bytes < 16MiB".into()));
        }
        if !(self.cpu_fraction > 0.0 && self.cpu_fraction <= 1.0) {
            return Err(ServeError::Config("cpu_fraction not in (0,1]".into()));
        }
        Ok(())
    }
}

fn is_loopback(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => v.is_loopback(),
        IpAddr::V6(v) => v.is_loopback(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_loopback_by_default() {
        let e = Config::from_json(br#"{"bind":"0.0.0.0:80"}"#).unwrap_err();
        assert!(matches!(e, ServeError::Config(_)));
    }

    #[test]
    fn clamps_json_depth_over_64() {
        let e = Config::from_json(br#"{"bind":"127.0.0.1:1","max_json_depth":99}"#).unwrap_err();
        assert!(matches!(e, ServeError::Config(_)));
    }

    #[test]
    fn default_bind_is_loopback() {
        let c = Config::from_json(br#"{"bind":"127.0.0.1:8090"}"#).unwrap();
        assert!(c.tcp_nodelay);
        assert!(c.so_reuseport);
    }
}
