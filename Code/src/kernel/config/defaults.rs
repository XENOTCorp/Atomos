//! Serde default constructors for Config.
use super::{MemoryMode, runtime_dir};
use std::path::PathBuf;

pub(crate) fn default_bind() -> String {
    "127.0.0.1:8090".into()
}
pub(crate) fn default_workers() -> u32 {
    super::physical_cpus().max(1)
}
pub(crate) fn default_backlog() -> i32 {
    1024
}
pub(crate) fn default_true() -> bool {
    true
}
pub(crate) fn default_false() -> bool {
    false
}
pub(crate) fn default_shutdown_ms() -> u64 {
    2000
}
pub(crate) fn default_wasm_fuel() -> u64 {
    10_000_000
}
pub(crate) fn default_wasm_memory() -> usize {
    16 * 1024 * 1024
}
pub(crate) fn default_ticket_secs() -> u64 {
    86400
}
pub(crate) fn default_keepalive() -> u64 {
    75
}
pub(crate) fn default_header_bytes() -> usize {
    16384
}
pub(crate) fn default_body_bytes() -> usize {
    262144
}
pub(crate) fn default_json_depth() -> u32 {
    32
}
pub(crate) fn default_timeout() -> u64 {
    45_000
}
pub(crate) fn default_header_timeout() -> u64 {
    10_000
}
pub(crate) fn default_body_timeout() -> u64 {
    30_000
}
pub(crate) fn default_idle_timeout() -> u64 {
    75_000
}
pub(crate) fn default_module_timeout() -> u64 {
    5_000
}
pub(crate) fn default_mem() -> u64 {
    6_000_000_000
}
pub(crate) fn default_mem_mode() -> MemoryMode {
    MemoryMode::Hard
}
pub(crate) fn default_cpu() -> f32 {
    0.40
}
pub(crate) fn default_cache_entries() -> usize {
    4096
}
pub(crate) fn default_cache_bytes() -> usize {
    16 * 1024 * 1024
}
pub(crate) fn default_rules() -> PathBuf {
    PathBuf::from("rules.json")
}
pub(crate) fn default_sock() -> PathBuf {
    runtime_dir().join("atomos.sock")
}

pub(crate) fn default_static() -> PathBuf {
    PathBuf::from("static")
}
pub(crate) fn default_error_page() -> PathBuf {
    PathBuf::from("static/error.html")
}
pub(crate) fn default_engine() -> String {
    "epoll".into()
}
