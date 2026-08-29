//! JSON config. Missing required bind is an error. Criticality C2.

use std::path::PathBuf;

use serde::Deserialize;

use crate::error::ServeError;

mod defaults;
mod host;
mod validate;

pub use host::{HostFacts, runtime_dir};

#[derive(Clone, Debug, Deserialize)]
pub struct Config {
    #[serde(default = "defaults::default_bind")]
    pub bind: String,
    #[serde(default)]
    pub allow_non_loopback: bool,
    #[serde(default = "defaults::default_workers")]
    pub workers: u32,
    #[serde(default = "defaults::default_backlog")]
    pub backlog: i32,
    #[serde(default = "defaults::default_true")]
    pub tcp_nodelay: bool,
    #[serde(default)]
    pub tcp_fastopen: bool,
    #[serde(default = "defaults::default_true")]
    pub so_reuseport: bool,
    #[serde(default = "defaults::default_keepalive")]
    pub keepalive_secs: u64,
    #[serde(default = "defaults::default_header_bytes")]
    pub max_header_bytes: usize,
    #[serde(default = "defaults::default_body_bytes")]
    pub max_body_bytes: usize,
    #[serde(default = "defaults::default_json_depth")]
    pub max_json_depth: u32,
    #[serde(default = "defaults::default_timeout")]
    pub request_timeout_ms: u64,
    /// Slowloris: close if headers are still incomplete after this.
    #[serde(default = "defaults::default_header_timeout")]
    pub header_timeout_ms: u64,
    #[serde(default = "defaults::default_body_timeout")]
    pub body_timeout_ms: u64,
    #[serde(default = "defaults::default_idle_timeout")]
    pub idle_timeout_ms: u64,
    /// After `handle` returns, 504 if the call ran longer than this.
    #[serde(default = "defaults::default_module_timeout")]
    pub module_timeout_ms: u64,
    #[serde(default = "defaults::default_mem")]
    pub memory_cap_bytes: u64,
    #[serde(default = "defaults::default_mem_mode")]
    pub memory_mode: MemoryMode,
    #[serde(default = "defaults::default_cpu")]
    pub cpu_fraction: f32,
    #[serde(default = "defaults::default_cache_entries")]
    pub cache_entries: usize,
    #[serde(default = "defaults::default_cache_bytes")]
    pub cache_bytes: usize,
    pub pre_module: Option<String>,
    pub post_module: Option<String>,
    #[serde(default = "defaults::default_rules")]
    pub rules_path: PathBuf,
    #[serde(default = "defaults::default_sock")]
    pub control_socket: PathBuf,
    #[serde(default = "defaults::default_static")]
    pub static_root: PathBuf,
    #[serde(default = "defaults::default_error_page")]
    pub error_page: PathBuf,
    #[serde(default = "defaults::default_true")]
    pub cpu_pin: bool,
    /// H2 is proto-process only. Default false so engine=epoll validates.
    #[serde(default = "defaults::default_false")]
    pub http2: bool,
    /// H3 is proto-process only. Default false so engine=epoll validates.
    #[serde(default = "defaults::default_false")]
    pub http3: bool,
    pub tls_cert: Option<PathBuf>,
    pub tls_key: Option<PathBuf>,
    /// Ports this process will not bind. Empty by default (no hardcoded list).
    #[serde(default)]
    pub refuse_ports: Vec<u16>,
    /// Directory of plugin manifests (`*.json`). See `plugin::load_dir`.
    pub plugin_dir: Option<PathBuf>,
    /// I/O engine: `tokio` (compiled), `epoll` / `xdp` (plug slots).
    #[serde(default = "defaults::default_engine")]
    pub engine: String,
    /// After bind, setuid/setgid if running as root.
    pub drop_user: Option<String>,
    pub drop_group: Option<String>,
    /// `prctl(NO_NEW_PRIVS)` after bind (Linux).
    #[serde(default = "defaults::default_true")]
    pub no_new_privs: bool,
    /// SIGTERM drain of keep-alives (ms). Used by `atomos-sup`.
    #[serde(default = "defaults::default_shutdown_ms")]
    pub worker_shutdown_timeout_ms: u64,
    /// Landlock FS restrict after bind (Linux). Off in unit tests unless set.
    #[serde(default)]
    pub landlock: bool,
    /// seccomp allowlist after bind (Linux). Off unless set.
    #[serde(default)]
    pub seccomp: bool,
    /// Drop remaining capabilities after setuid (Linux).
    #[serde(default = "defaults::default_true")]
    pub drop_caps: bool,
    /// Access log after encode (effect adapter). Off the cache-hit predicate.
    #[serde(default)]
    pub access_log: bool,
    /// Wasm fuel units per `handle`. Default 10_000_000.
    #[serde(default = "defaults::default_wasm_fuel")]
    pub wasm_fuel: u64,
    /// OCSP staple DER/file. Read at TLS load; never fetched on GET.
    pub tls_ocsp: Option<PathBuf>,
    /// Ticket lifetime seconds for rustls. 0 = rustls default.
    #[serde(default = "defaults::default_ticket_secs")]
    pub tls_ticket_lifetime_secs: u64,
    /// Unix socket for `atomos-keyd`. If set, workers must not load tls_key.
    pub keyd_sock: Option<PathBuf>,
    /// Integer scheduler: rule mode + limits (see `crate::sched`).
    #[serde(default)]
    pub scheduler: SchedConfig,
}

/// JSON block for the admission scheduler (integer-only).
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase", default)]
pub struct SchedConfig {
    pub mode: crate::sched::RuleMode,
    pub demand_limit: i32,
    pub q_max: u32,
    pub q_per_ip: u32,
    pub c_max: u32,
    pub c_per_ip: u32,
    pub b_max: u32,
    pub str_max: u32,
    pub custom_div: i32,
    pub custom_dem: i32,
    pub custom_exc: i32,
    pub custom_wait: i32,
    pub custom_qpen: i32,
    pub custom_cpen: i32,
}

impl Default for SchedConfig {
    fn default() -> Self {
        let l = crate::sched::Limits::default();
        let w = crate::sched::Weights::default();
        Self {
            mode: crate::sched::RuleMode::default(),
            demand_limit: l.d_limit,
            q_max: l.q_max,
            q_per_ip: l.q_per_ip,
            c_max: l.c_max,
            c_per_ip: l.c_per_ip,
            b_max: l.b_max,
            str_max: l.str_max,
            custom_div: w.div,
            custom_dem: w.dem,
            custom_exc: w.exc,
            custom_wait: w.wait,
            custom_qpen: w.qpen,
            custom_cpen: w.cpen,
        }
    }
}

impl SchedConfig {
    /// Build the runtime `Limits` + `Weights` from this config block.
    pub fn build(&self) -> (crate::sched::Limits, crate::sched::Weights) {
        let limits = crate::sched::Limits {
            c_max: self.c_max,
            c_per_ip: self.c_per_ip,
            q_max: self.q_max,
            q_per_ip: self.q_per_ip,
            b_max: self.b_max,
            h_max: 65_536,
            s_max: 10 << 20,
            str_max: self.str_max,
            d_limit: self.demand_limit,
        };
        let custom = crate::sched::Weights {
            div: self.custom_div,
            dem: self.custom_dem,
            exc: self.custom_exc,
            wait: self.custom_wait,
            qpen: self.custom_qpen,
            cpen: self.custom_cpen,
        };
        (limits, custom)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MemoryMode {
    Hard,
    Degrade,
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
        let mut c = Self::from_json(&raw)?;
        c.apply_host_file();
        Ok(c)
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
    fn default_engine_is_epoll() {
        let c = Config::from_json(br#"{"bind":"127.0.0.1:0","memory_cap_bytes":67108864}"#).unwrap();
        assert_eq!(c.engine, "epoll");
        assert!(!c.http2);
        assert!(!c.http3);
    }

    #[test]
    fn epoll_with_http2_is_config_error() {
        let e = Config::from_json(
            br#"{"bind":"127.0.0.1:0","memory_cap_bytes":67108864,"engine":"epoll","http2":true,"http3":false}"#,
        )
        .unwrap_err();
        let s = e.to_string();
        assert!(s.contains("epoll"), "{s}");
        assert!(s.contains("http2") || s.contains("engine"), "{s}");
    }

    #[test]
    fn shutdown_timeout_from_config() {
        let c = Config::from_json(
            br#"{"bind":"127.0.0.1:0","memory_cap_bytes":67108864,"worker_shutdown_timeout_ms":1500}"#,
        )
        .unwrap();
        assert_eq!(c.worker_shutdown_timeout_ms, 1500);
    }

    #[test]
    fn tokio_may_enable_http2() {
        Config::from_json(
            br#"{"bind":"127.0.0.1:0","memory_cap_bytes":67108864,"engine":"tokio","http2":true,"http3":false}"#,
        )
        .unwrap();
    }

    #[test]
    fn default_bind_is_loopback() {
        let c = Config::from_json(br#"{"bind":"127.0.0.1:8090"}"#).unwrap();
        assert!(c.tcp_nodelay);
        assert!(c.so_reuseport);
        assert!(c.cpu_pin);
        assert!(!c.http2);
        assert!(!c.http3);
        assert_eq!(c.engine, "epoll");
        assert!(c.refuse_ports.is_empty());
        assert!(c.no_new_privs);
        let sock = c.control_socket.to_string_lossy();
        assert!(
            sock.contains("atomos.sock"),
            "{sock}"
        );
    }

    #[test]
    fn overlay_host_sets_workers_and_refuse() {
        let mut c = Config::from_json(br#"{"bind":"127.0.0.1:8090"}"#).unwrap();
        c.overlay_host(&HostFacts {
            workers: Some(8),
            cpu_pin: Some(true),
            cache_bytes: Some(4096),
            cache_entries: None,
            refuse_ports: Some(vec![9]),
        });
        assert_eq!(c.workers, 8);
        assert_eq!(c.refuse_ports, vec![9]);
        assert_eq!(c.cache_bytes, 4096);
    }
}
