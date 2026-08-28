//! Minimal consumer: static files on loopback.

use atomos::config::Config;
use atomos::rules::Ruleset;
use atomos::{serve, static_router};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let root = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "static".into());
    let bind = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "127.0.0.1:0".into());
    let cfg = Config::from_json(
        format!(
            r#"{{"bind":"{bind}","static_root":"{root}","memory_cap_bytes":6000000000,"so_reuseport":true,"tcp_nodelay":true}}"#
        )
        .as_bytes(),
    )
    .expect("config");
    let rules = Ruleset::parse(
        br#"{"rules":[{"id":"s","module":"static","methods":["GET","HEAD"],"include":["/*"],"exclude":[]}]}"#,
    )
    .expect("rules");
    let (router, ctx, _) = static_router(cfg, rules);
    if let Err(e) = serve::run(router, ctx).await {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
