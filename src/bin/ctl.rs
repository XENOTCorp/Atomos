//! atomos-ctl — operator TUI. Separate process from the HTTP server.

use std::path::PathBuf;

use atomos::config::Config;
use atomos::tui;

fn default_config() -> PathBuf {
    std::env::var("ATOMOS_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("config.json"))
}

fn arg(args: &[String], name: &str) -> Option<String> {
    args.windows(2).find_map(|w| {
        if w[0] == name {
            Some(w[1].clone())
        } else {
            None
        }
    })
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("install-link") {
        let home = std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("."));
        let target = std::env::current_exe().expect("exe");
        match tui::install_link(&home, &target) {
            Ok(p) => println!("{}", p.display()),
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
        return;
    }
    let path = arg(&args, "--config")
        .map(PathBuf::from)
        .unwrap_or_else(default_config);
    let cfg = if path.exists() {
        Config::load_path(&path).unwrap_or_else(|e| {
            eprintln!("{e}");
            std::process::exit(1);
        })
    } else {
        Config::from_json(br#"{"bind":"127.0.0.1:8090"}"#).expect("cfg")
    };
    let json = arg(&args, "--json")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("data.json"));
    if let Err(e) = tui::run_tui(&cfg, json) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn bin_name_is_ctl_not_server() {
        assert_eq!(env!("CARGO_BIN_NAME"), "atomos-ctl");
    }
}
