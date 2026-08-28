//! `atomos-sup` — spawn pinned workers of `atomos` / `first_app`. Does not bind HTTP.

use atomos::config::Config;
use atomos::sup::{self, WorkerSpec};

fn main() {
    let n = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| {
            Config::from_json(br#"{"bind":"127.0.0.1:8090"}"#)
                .map(|c| c.workers)
                .unwrap_or(2)
        });
    let exe = std::env::args()
        .nth(2)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::current_exe().expect("exe").with_file_name("atomos"));
    let rest: Vec<String> = std::env::args().skip(3).collect();
    if let Err(e) = sup::run(WorkerSpec {
        exe,
        args: rest,
        n,
        shutdown_timeout: std::time::Duration::from_millis(
            Config::from_json(br#"{"bind":"127.0.0.1:8090"}"#)
                .map(|c| c.worker_shutdown_timeout_ms)
                .unwrap_or(2000),
        ),
    }) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
