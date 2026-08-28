//! atomos-ctl — operator CLI / JSON API. Separate process from the HTTP server.

use std::io::IsTerminal;
use std::path::PathBuf;

use atomos::config::Config;
use atomos::ctl::{self, Env};

fn default_config() -> PathBuf {
    std::env::var("ATOMOS_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("config.json"))
}

fn looks_like_path(s: &str) -> bool {
    s.contains('/') || s.ends_with(".json") || s.starts_with('.')
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("install-link") {
        let home = std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("."));
        let target = std::env::current_exe().expect("exe");
        match ctl::install_link(&home, &target) {
            Ok(p) => println!("{}", p.display()),
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
        return;
    }

    let mut json = false;
    let mut config: Option<PathBuf> = None;
    let mut data: Option<PathBuf> = None;
    let mut socket: Option<PathBuf> = None;
    let mut words: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--json" => {
                // Old flag: `--json FILE` was the CRUD path. New flag: JSON-lines mode.
                if let Some(next) = args.get(i + 1) {
                    if looks_like_path(next) && !next.starts_with('-') {
                        i += 1;
                        data = Some(PathBuf::from(&args[i]));
                        continue;
                    }
                }
                json = true;
            }
            "--data" => {
                i += 1;
                match args.get(i) {
                    Some(p) => data = Some(PathBuf::from(p)),
                    None => {
                        eprintln!("--data needs a path");
                        std::process::exit(2);
                    }
                }
            }
            "--config" => {
                i += 1;
                match args.get(i) {
                    Some(p) => config = Some(PathBuf::from(p)),
                    None => {
                        eprintln!("--config needs a path");
                        std::process::exit(2);
                    }
                }
            }
            "--socket" => {
                i += 1;
                match args.get(i) {
                    Some(p) => socket = Some(PathBuf::from(p)),
                    None => {
                        eprintln!("--socket needs a path");
                        std::process::exit(2);
                    }
                }
            }
            "-h" | "--help" => {
                words.clear();
                words.push("help".into());
            }
            other => words.push(other.to_string()),
        }
        i += 1;
    }

    let path = config.unwrap_or_else(default_config);
    let mut cfg = if path.exists() {
        Config::load_path(&path).unwrap_or_else(|e| {
            eprintln!("{e}");
            std::process::exit(1);
        })
    } else {
        Config::from_json(br#"{"bind":"127.0.0.1:8090"}"#).expect("cfg")
    };
    if let Some(s) = socket {
        cfg.control_socket = s;
    }
    let data_path = data.unwrap_or_else(|| PathBuf::from("data.json"));

    let env = Env { cfg, data_path };
    let tty = std::io::stdin().is_terminal();
    std::process::exit(ctl::run_cli(&env, &words, json, tty));
}

#[cfg(test)]
mod tests {
    #[test]
    fn bin_name_is_ctl_not_server() {
        assert_eq!(env!("CARGO_BIN_NAME"), "atomos-ctl");
    }
}
