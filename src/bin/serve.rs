//! atomos — static (and registered) HTTP server. Loopback by default.

use std::path::PathBuf;

use atomos::config::Config;
use atomos::control;
use atomos::rules::Ruleset;
use atomos::{serve, static_router};

fn arg(args: &[String], name: &str) -> Option<String> {
    args.windows(2).find_map(|w| {
        if w[0] == name {
            Some(w[1].clone())
        } else {
            None
        }
    })
}

fn default_config() -> PathBuf {
    std::env::var("ATOMOS_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("config.json"))
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("-h")
        || args.first().map(String::as_str) == Some("--help")
    {
        eprintln!(
            "atomos [--config FILE] [--bind HOST:PORT] [--root DIR] [--rules FILE]\nDefault bind 127.0.0.1:8090. Does not bind 8082."
        );
        std::process::exit(0);
    }
    let cfg_path = arg(&args, "--config")
        .map(PathBuf::from)
        .unwrap_or_else(default_config);

    let mut cfg = if cfg_path.exists() {
        Config::load_path(&cfg_path).unwrap_or_else(|e| {
            eprintln!("config: {e}");
            std::process::exit(1);
        })
    } else {
        Config::from_json(br#"{"bind":"127.0.0.1:8090"}"#).expect("default config")
    };
    if let Some(b) = arg(&args, "--bind") {
        cfg.bind = b;
        if let Err(e) = cfg.validate() {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
    if let Some(root) = arg(&args, "--root") {
        cfg.static_root = PathBuf::from(root);
    }
    let rules_path = arg(&args, "--rules")
        .map(PathBuf::from)
        .unwrap_or(cfg.rules_path.clone());
    let rules = if rules_path.exists() {
        let raw = std::fs::read(&rules_path).unwrap_or_else(|e| {
            eprintln!("rules: {e}");
            std::process::exit(1);
        });
        Ruleset::parse(&raw).unwrap_or_else(|e| {
            eprintln!("rules: {e}");
            std::process::exit(1);
        })
    } else {
        Ruleset::parse(
            br#"{"rules":[{"id":"s","module":"static","methods":["GET","HEAD"],"include":["/*"],"exclude":[]}]}"#,
        )
        .expect("builtin rules")
    };

    let sock = cfg.control_socket.clone();
    let (router, ctx, _) = static_router(cfg, rules);
    let ctl = ctx.clone();
    tokio::spawn(async move {
        if let Err(e) = control::serve_control(sock, ctl).await {
            tracing::warn!(%e, "control socket");
        }
    });
    if let Err(e) = serve::run(router, ctx).await {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
