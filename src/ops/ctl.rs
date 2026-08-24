//! Operator CLI / JSON API. Separate process from the HTTP listener.
//! File mutations go through atoms only. Criticality C2 (CRUD) / C1 (display).

use std::io::{BufRead, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};

use crate::atom::AtomCtx;
use crate::config::Config;
use crate::error::AtomError;

/// `$HOME/atomos` → `target` (usually this ctl binary).
pub fn install_link(home: &Path, target: &Path) -> std::io::Result<PathBuf> {
    let link = home.join("atomos");
    if link.exists() || link.symlink_metadata().is_ok() {
        let _ = std::fs::remove_file(&link);
    }
    std::os::unix::fs::symlink(target, &link)?;
    Ok(link)
}

#[derive(Debug, Clone)]
pub struct Env {
    pub cfg: Config,
    pub data_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cmd {
    Help,
    Status,
    KeysList { reveal: bool },
    KeysAdd { note: String },
    KeysDel { index: usize, yes: bool },
    JsonDump,
    ConfigShow,
    Restart { yes: bool },
    Stop { yes: bool },
    Start,
    Refresh,
    Backup,
    DryTest,
    Quit,
}

pub fn io_error_json(err: &std::io::Error, path: &Path) -> Value {
    let path_s = path.display().to_string();
    match err.kind() {
        std::io::ErrorKind::PermissionDenied => json!({
            "ok": false,
            "error": "permission_denied",
            "path": path_s,
            "message": format!("permission denied opening {path_s} (mode 0600, same EUID only)")
        }),
        std::io::ErrorKind::NotFound if path_s.ends_with(".sock") => json!({
            "ok": false,
            "error": "server_unreachable",
            "path": path_s,
            "message": format!("control Unix socket {path_s} is missing (not a TCP port; HTTP bind is separate)")
        }),
        std::io::ErrorKind::NotFound => json!({
            "ok": false,
            "error": "not_found",
            "path": path_s,
            "message": format!("not found: {path_s}")
        }),
        std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::ConnectionReset => json!({
            "ok": false,
            "error": "server_unreachable",
            "path": path_s,
            "message": format!("nothing is listening on {path_s} (stale socket file?)")
        }),
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => json!({
            "ok": false,
            "error": "timeout",
            "path": path_s,
            "message": format!("timeout talking to {path_s}")
        }),
        _ => json!({
            "ok": false,
            "error": "io",
            "path": path_s,
            "message": format!("{path_s}: {err}")
        }),
    }
}

fn connect_error_json(err: &std::io::Error, path: &Path) -> Value {
    let mut v = io_error_json(err, path);
    if v["error"] == "not_found" {
        v["error"] = json!("server_unreachable");
        v["message"] = json!(format!(
            "control Unix socket {} is missing (not a TCP port; HTTP bind is separate)",
            path.display()
        ));
    }
    v
}

pub fn atom_error_json(err: AtomError, path: &Path) -> Value {
    match err {
        AtomError::Io(e) => io_error_json(&e, path),
        other => json!({"ok": false, "error": "atom", "message": other.to_string()}),
    }
}

pub fn atom_err_msg(err: AtomError, path: &Path) -> String {
    atom_error_json(err, path)
        .get("message")
        .and_then(|m| m.as_str())
        .unwrap_or("error")
        .to_string()
}

pub fn http_listening(bind: &str) -> bool {
    let Ok(addr) = bind.parse::<SocketAddr>() else {
        return false;
    };
    TcpStream::connect_timeout(&addr, Duration::from_millis(150)).is_ok()
}

fn uds_raw(sock: &Path, cmd: &str) -> Result<Value, Value> {
    let mut s = match UnixStream::connect(sock) {
        Ok(s) => s,
        Err(e) => return Err(connect_error_json(&e, sock)),
    };
    let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = s.set_write_timeout(Some(Duration::from_secs(2)));
    let line = format!("{{\"cmd\":\"{cmd}\"}}\n");
    if let Err(e) = s.write_all(line.as_bytes()) {
        return Err(connect_error_json(&e, sock));
    }
    let mut buf = Vec::new();
    if let Err(e) = s.read_to_end(&mut buf) {
        return Err(connect_error_json(&e, sock));
    }
    serde_json::from_slice(&buf)
        .map_err(|e| json!({"ok": false, "error": "json", "message": e.to_string()}))
}

pub fn uds_cmd(sock: &Path, cmd: &str) -> Result<Value, String> {
    match uds_raw(sock, cmd) {
        Ok(v) => Ok(v),
        Err(e) => Err(e
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("server unreachable")
            .to_string()),
    }
}

pub fn help_text() -> String {
    "ATOMOS operator ctl

Usage:
  atomos-ctl [--config PATH] [--data PATH] [--json] <command> [args]
  atomos-ctl                          interactive prompt  >
  atomos-ctl --json                   JSON lines on stdin → JSON lines on stdout

Commands:
  help                 this text
  status | ping        control Unix socket + HTTP bind liveness
  keys list            list /keys in the data file (masked unless --reveal)
  keys add [note]      append a random token via json.crud
  keys del <index>     delete key at index (needs --yes)
  json dump            pretty-print the data file
  config               bind, socket, rules_path, static_root
  start | stop | restart | refresh | backup | dry-test
  quit                 leave the prompt

Flags:
  --config PATH        ATOMOS_CONFIG or config.json
  --data PATH          JSON file for keys CRUD (default data.json)
  --socket PATH        override control Unix socket
  --json               machine envelope (one JSON object per line)
  --yes                confirm destructive commands
  --reveal             show full tokens
  -h, --help

JSON line API (GUI / scripts):
  {\"cmd\":\"status\"}
  {\"cmd\":\"keys.list\",\"reveal\":false}
  {\"cmd\":\"keys.add\",\"note\":\"lab\"}
  {\"cmd\":\"keys.del\",\"index\":0,\"yes\":true}
  {\"cmd\":\"json.dump\"}
  {\"cmd\":\"refresh\"}
  {\"cmd\":\"restart\",\"yes\":true}

Control plane is the Unix socket ($XDG_RUNTIME_DIR/atomos.sock), not the HTTP bind.
"
    .to_string()
}

pub fn parse_line(line: &str) -> Result<Cmd, String> {
    let t = line.trim();
    if t.is_empty() {
        return Err("empty".into());
    }
    if t.starts_with('{') {
        let v: Value = serde_json::from_str(t).map_err(|e| e.to_string())?;
        return parse_json(&v);
    }
    let words: Vec<&str> = t.split_whitespace().collect();
    parse_words(&words)
}

pub fn parse_words<S: AsRef<str>>(words: &[S]) -> Result<Cmd, String> {
    let mut yes = false;
    let mut reveal = false;
    let mut w: Vec<&str> = Vec::new();
    for x in words {
        match x.as_ref() {
            "--yes" | "-y" => yes = true,
            "--reveal" => reveal = true,
            s => w.push(s),
        }
    }
    if w.is_empty() {
        return Err("empty".into());
    }
    match w[0] {
        "help" | "-h" | "--help" => Ok(Cmd::Help),
        "status" | "ping" => Ok(Cmd::Status),
        "quit" | "exit" | "q" => Ok(Cmd::Quit),
        "config" => Ok(Cmd::ConfigShow),
        "json" => match w.get(1).copied().unwrap_or("dump") {
            "dump" | "pretty" | "show" => Ok(Cmd::JsonDump),
            "list" | "ls" => Ok(Cmd::KeysList { reveal }),
            other => Err(format!("unknown json subcommand: {other}")),
        },
        "keys" => {
            let sub = w.get(1).copied().unwrap_or("list");
            match sub {
                "list" | "ls" => Ok(Cmd::KeysList { reveal }),
                "add" => Ok(Cmd::KeysAdd {
                    note: w.get(2..).unwrap_or(&[]).join(" "),
                }),
                "del" | "rm" | "delete" => {
                    let index = w
                        .get(2)
                        .ok_or_else(|| "keys del <index>".to_string())?
                        .parse::<usize>()
                        .map_err(|_| "keys del <index> must be a number".to_string())?;
                    Ok(Cmd::KeysDel { index, yes })
                }
                other => Err(format!("unknown keys subcommand: {other}")),
            }
        }
        "restart" => Ok(Cmd::Restart { yes }),
        "stop" => Ok(Cmd::Stop { yes }),
        "start" => Ok(Cmd::Start),
        "refresh" | "refresh-endpoints" => Ok(Cmd::Refresh),
        "backup" => Ok(Cmd::Backup),
        "dry-test" | "dry_test" | "dry-test-rules" => Ok(Cmd::DryTest),
        other => Err(format!("unknown command: {other} (try help)")),
    }
}

pub fn parse_json(v: &Value) -> Result<Cmd, String> {
    let cmd = v
        .get("cmd")
        .and_then(|c| c.as_str())
        .ok_or_else(|| "missing cmd".to_string())?;
    let yes = v.get("yes").and_then(|x| x.as_bool()).unwrap_or(false);
    let reveal = v.get("reveal").and_then(|x| x.as_bool()).unwrap_or(false);
    let note = v
        .get("note")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let index = v.get("index").and_then(|x| x.as_u64()).map(|n| n as usize);
    match cmd {
        "help" => Ok(Cmd::Help),
        "status" | "ping" => Ok(Cmd::Status),
        "quit" | "exit" => Ok(Cmd::Quit),
        "config" => Ok(Cmd::ConfigShow),
        "json.dump" | "json_dump" | "json" => Ok(Cmd::JsonDump),
        "keys.list" | "keys_list" => Ok(Cmd::KeysList { reveal }),
        "keys.add" | "keys_add" => Ok(Cmd::KeysAdd { note }),
        "keys.del" | "keys_del" => Ok(Cmd::KeysDel {
            index: index.ok_or_else(|| "keys.del needs index".to_string())?,
            yes,
        }),
        "keys" => match v.get("op").and_then(|o| o.as_str()).unwrap_or("list") {
            "list" => Ok(Cmd::KeysList { reveal }),
            "add" => Ok(Cmd::KeysAdd { note }),
            "del" => Ok(Cmd::KeysDel {
                index: index.ok_or_else(|| "keys del needs index".to_string())?,
                yes,
            }),
            other => Err(format!("unknown keys op: {other}")),
        },
        "restart" => Ok(Cmd::Restart { yes }),
        "stop" => Ok(Cmd::Stop { yes }),
        "start" => Ok(Cmd::Start),
        "refresh" | "refresh-endpoints" => Ok(Cmd::Refresh),
        "backup" => Ok(Cmd::Backup),
        "dry-test" | "dry_test" | "dry-test-rules" => Ok(Cmd::DryTest),
        other => Err(format!("unknown cmd: {other}")),
    }
}

fn confirm_needed(what: &str) -> Value {
    json!({
        "ok": false,
        "error": "confirm",
        "message": format!("{what} requires --yes (JSON: {{\"cmd\":\"{what}\",\"yes\":true}})")
    })
}

fn enrich_http(mut v: Value, cfg: &Config) -> Value {
    let listening = http_listening(&cfg.bind);
    v["http"] = json!({ "bind": cfg.bind, "listening": listening });
    v["socket"] = json!(cfg.control_socket.display().to_string());
    if v.get("ok").and_then(|o| o.as_bool()) != Some(true) && listening {
        v["hint"] = json!(format!(
            "HTTP {} is listening. Control is Unix socket {}, not the HTTP bind. The process may be an old binary without a control socket. ctl will not bind HTTP or spawn a second server.",
            cfg.bind,
            cfg.control_socket.display()
        ));
    }
    v
}

fn now_s() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn random_token() -> String {
    let mut b = [0u8; 12];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let _ = f.read_exact(&mut b);
    }
    let mut out = String::with_capacity(24);
    for x in b {
        let _ = std::fmt::Write::write_fmt(&mut out, format_args!("{x:02x}"));
    }
    out
}

fn mask(s: &str) -> String {
    if s.len() <= 4 {
        "****".into()
    } else {
        format!("{}…{}", &s[..2], &s[s.len() - 2..])
    }
}

pub fn exec_cmd(env: &Env, cmd: Cmd) -> Value {
    let cfg = &env.cfg;
    match cmd {
        Cmd::Help => json!({"ok": true, "help": help_text()}),
        Cmd::Quit => json!({"ok": true, "quit": true}),
        Cmd::Status => match uds_raw(&cfg.control_socket, "status") {
            Ok(control) => enrich_http(json!({"ok": true, "control": control}), cfg),
            Err(e) => enrich_http(e, cfg),
        },
        Cmd::ConfigShow => json!({
            "ok": true,
            "bind": cfg.bind,
            "control_socket": cfg.control_socket.display().to_string(),
            "rules_path": cfg.rules_path.display().to_string(),
            "static_root": cfg.static_root.display().to_string(),
            "data": env.data_path.display().to_string(),
            "pre_module": cfg.pre_module,
            "post_module": cfg.post_module,
            "cache_entries": cfg.cache_entries,
            "memory_cap_bytes": cfg.memory_cap_bytes,
        }),
        Cmd::JsonDump => match std::fs::read(&env.data_path) {
            Ok(raw) => match serde_json::from_slice::<Value>(&raw) {
                Ok(doc) => json!({"ok": true, "data": doc}),
                Err(e) => json!({"ok": false, "error": "json", "message": e.to_string()}),
            },
            Err(e) => io_error_json(&e, &env.data_path),
        },
        Cmd::KeysList { reveal } => keys_list(&env.data_path, reveal),
        Cmd::KeysAdd { note } => {
            let tok = random_token();
            let ctx = AtomCtx::test();
            match ctx.run(
                "json.crud",
                json!({
                    "path": env.data_path.display().to_string(),
                    "op": "add",
                    "pointer": "/keys/-",
                    "value": { "token": tok.clone(), "note": note.clone(), "created_s": now_s() }
                }),
            ) {
                Ok(v) => {
                    if v.get("ok") == Some(&Value::Bool(false)) {
                        v
                    } else {
                        json!({"ok": true, "token": tok, "note": note})
                    }
                }
                Err(e) => atom_error_json(e, &env.data_path),
            }
        }
        Cmd::KeysDel { index, yes } => {
            if !yes {
                return confirm_needed("keys.del");
            }
            let ctx = AtomCtx::test();
            match ctx.run(
                "json.crud",
                json!({
                    "path": env.data_path.display().to_string(),
                    "op": "del",
                    "pointer": format!("/keys/{index}")
                }),
            ) {
                Ok(v) => {
                    if v.get("ok") == Some(&Value::Bool(false)) {
                        v
                    } else {
                        json!({"ok": true, "deleted": index})
                    }
                }
                Err(e) => atom_error_json(e, &env.data_path),
            }
        }
        Cmd::Restart { yes } => {
            if !yes {
                return confirm_needed("restart");
            }
            uds_wrap(cfg, "restart")
        }
        Cmd::Stop { yes } => {
            if !yes {
                return confirm_needed("stop");
            }
            uds_wrap(cfg, "stop")
        }
        Cmd::Start => uds_wrap(cfg, "start"),
        Cmd::Refresh => uds_wrap(cfg, "refresh-endpoints"),
        Cmd::Backup => {
            let dest = env.data_path.with_extension("bak");
            let ctx = AtomCtx::test();
            match ctx.run(
                "settings.backup",
                json!({
                    "path": env.data_path.display().to_string(),
                    "dest": dest.display().to_string()
                }),
            ) {
                Ok(v) => json!({"ok": true, "dest": dest.display().to_string(), "data": v}),
                Err(e) => atom_error_json(e, &env.data_path),
            }
        }
        Cmd::DryTest => {
            let ctx = AtomCtx::test();
            let input = if cfg.rules_path.exists() {
                json!({"path": cfg.rules_path.display().to_string()})
            } else {
                json!({})
            };
            match ctx.run("rules.dry_test", input) {
                Ok(v) => json!({"ok": true, "data": v}),
                Err(e) => atom_error_json(e, &cfg.rules_path),
            }
        }
    }
}

fn uds_wrap(cfg: &Config, cmd: &str) -> Value {
    match uds_raw(&cfg.control_socket, cmd) {
        Ok(control) => enrich_http(json!({"ok": true, "control": control}), cfg),
        Err(e) => enrich_http(e, cfg),
    }
}

fn keys_list(path: &Path, reveal: bool) -> Value {
    if !path.exists() {
        return json!({"ok": true, "keys": []});
    }
    match std::fs::read(path) {
        Err(e) => io_error_json(&e, path),
        Ok(raw) => match serde_json::from_slice::<Value>(&raw) {
            Err(e) => json!({"ok": false, "error": "json", "message": e.to_string()}),
            Ok(doc) => {
                let keys = match doc.get("keys").and_then(|k| k.as_array()) {
                    Some(arr) => arr
                        .iter()
                        .enumerate()
                        .map(|(i, v)| {
                            if let Some(t) = v.get("token").and_then(|x| x.as_str()) {
                                let shown = if reveal {
                                    t.to_string()
                                } else {
                                    mask(t)
                                };
                                json!({
                                    "index": i,
                                    "token": shown,
                                    "note": v.get("note").and_then(|x| x.as_str()).unwrap_or(""),
                                    "created_s": v.get("created_s").and_then(|x| x.as_u64()).unwrap_or(0)
                                })
                            } else {
                                json!({"index": i, "value": v})
                            }
                        })
                        .collect::<Vec<_>>(),
                    None => Vec::new(),
                };
                json!({"ok": true, "keys": keys})
            }
        },
    }
}

pub fn format_human(v: &Value) -> String {
    if let Some(h) = v.get("help").and_then(|x| x.as_str()) {
        return h.to_string();
    }
    if v.get("ok").and_then(|o| o.as_bool()) != Some(true) {
        let err = v.get("error").and_then(|x| x.as_str()).unwrap_or("error");
        let msg = v.get("message").and_then(|x| x.as_str()).unwrap_or("");
        let mut s = if msg.is_empty() {
            err.to_string()
        } else {
            format!("{err}: {msg}")
        };
        if let Some(h) = v.get("hint").and_then(|x| x.as_str()) {
            s.push('\n');
            s.push_str(h);
        }
        return s;
    }
    serde_json::to_string_pretty(v).unwrap_or_else(|_| "{}".into())
}

pub fn run_repl<R: BufRead, W: Write>(env: &Env, mut input: R, mut out: W) -> std::io::Result<()> {
    writeln!(
        out,
        "ATOMOS ctl  socket={}  bind={}  data={}",
        env.cfg.control_socket.display(),
        env.cfg.bind,
        env.data_path.display()
    )?;
    writeln!(out, "type help  ·  JSON objects also accepted")?;
    loop {
        write!(out, "> ")?;
        out.flush()?;
        let mut line = String::new();
        if input.read_line(&mut line)? == 0 {
            writeln!(out)?;
            break;
        }
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        match parse_line(t) {
            Ok(Cmd::Quit) => break,
            Ok(cmd) => {
                let v = exec_cmd(env, cmd);
                writeln!(out, "{}", format_human(&v).trim_end())?;
            }
            Err(e) => writeln!(out, "usage: {e}")?,
        }
    }
    Ok(())
}

pub fn run_json_lines<R: BufRead, W: Write>(
    env: &Env,
    input: R,
    mut out: W,
) -> std::io::Result<()> {
    for line in input.lines() {
        let line = line?;
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        let parsed = if t.starts_with('{') {
            match serde_json::from_str::<Value>(t) {
                Ok(v) => parse_json(&v),
                Err(e) => Err(e.to_string()),
            }
        } else {
            parse_line(t)
        };
        let v = match parsed {
            Ok(Cmd::Quit) => {
                writeln!(out, "{}", json!({"ok": true, "quit": true}))?;
                break;
            }
            Ok(c) => exec_cmd(env, c),
            Err(e) => json!({"ok": false, "error": "usage", "message": e}),
        };
        writeln!(out, "{v}")?;
    }
    Ok(())
}

pub fn run_cli(env: &Env, words: &[String], json: bool, stdin_tty: bool) -> i32 {
    if words.is_empty() {
        if json || !stdin_tty {
            if let Err(e) = run_json_lines(env, std::io::stdin().lock(), std::io::stdout()) {
                eprintln!("{e}");
                return 1;
            }
            return 0;
        }
        if let Err(e) = run_repl(env, std::io::stdin().lock(), std::io::stdout()) {
            eprintln!("{e}");
            return 1;
        }
        return 0;
    }
    match parse_words(words) {
        Ok(Cmd::Quit) => 0,
        Ok(cmd) => {
            let v = exec_cmd(env, cmd);
            if json {
                println!("{v}");
            } else {
                println!("{}", format_human(&v).trim_end());
            }
            if v.get("ok").and_then(|o| o.as_bool()) == Some(true) {
                0
            } else {
                1
            }
        }
        Err(e) => {
            let v = json!({"ok": false, "error": "usage", "message": e});
            if json {
                println!("{v}");
            } else {
                eprintln!("{}", format_human(&v));
            }
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn permission_denied_is_named_not_raw_os() {
        let e = std::io::Error::from_raw_os_error(13);
        let v = io_error_json(&e, Path::new("/tmp/atomos.sock"));
        assert_eq!(v["ok"], false);
        assert_eq!(v["error"], "permission_denied");
        let msg = v["message"].as_str().unwrap();
        assert!(msg.contains("permission denied"), "{msg}");
        assert!(!msg.contains("os error"), "{msg}");
    }

    #[test]
    fn missing_socket_names_unix_path_not_tcp_port() {
        let e = std::io::Error::new(std::io::ErrorKind::NotFound, "No such file or directory");
        let v = io_error_json(&e, Path::new("/tmp/atomos.sock"));
        assert_eq!(v["error"], "server_unreachable");
        let msg = v["message"].as_str().unwrap();
        assert!(msg.contains("/tmp/atomos.sock"), "{msg}");
        assert!(msg.contains("Unix") || msg.contains("unix"), "{msg}");
        assert!(!msg.contains("8089"), "{msg}");
    }

    #[test]
    fn parse_repl_and_argv_commands() {
        assert_eq!(parse_words(&["help"]).unwrap(), Cmd::Help);
        assert_eq!(parse_words(&["status"]).unwrap(), Cmd::Status);
        assert_eq!(
            parse_words(&["keys", "add", "lab"]).unwrap(),
            Cmd::KeysAdd { note: "lab".into() }
        );
        assert_eq!(
            parse_words(&["keys", "del", "2", "--yes"]).unwrap(),
            Cmd::KeysDel {
                index: 2,
                yes: true
            }
        );
        assert_eq!(parse_line("json dump").unwrap(), Cmd::JsonDump);
        assert!(parse_words(&["nope"]).is_err());
    }

    #[test]
    fn json_object_is_a_command() {
        assert_eq!(parse_json(&json!({"cmd": "status"})).unwrap(), Cmd::Status);
        assert_eq!(
            parse_json(&json!({"cmd": "keys.add", "note": "lab"})).unwrap(),
            Cmd::KeysAdd { note: "lab".into() }
        );
    }

    #[test]
    fn help_text_lists_json_api_and_unix_socket() {
        let h = help_text();
        assert!(h.contains("status"), "{h}");
        assert!(h.contains("--json"), "{h}");
        assert!(h.contains("atomos.sock") || h.contains("Unix"), "{h}");
        assert!(h.contains("HTTP bind") || h.contains("Unix socket"), "{h}");
    }

    fn test_env(dir: &std::path::Path) -> Env {
        let data = dir.join("data.json");
        std::fs::write(&data, r#"{"keys":[]}"#).unwrap();
        let mut cfg =
            Config::from_json(br#"{"bind":"127.0.0.1:1","memory_cap_bytes":67108864}"#).unwrap();
        cfg.control_socket = dir.join("missing.sock");
        Env {
            cfg,
            data_path: data,
        }
    }

    #[test]
    fn keys_add_on_unreadable_file_is_permission_denied() {
        let dir = tempfile::tempdir().unwrap();
        let env = test_env(dir.path());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&env.data_path, std::fs::Permissions::from_mode(0o000))
                .unwrap();
        }
        let v = exec_cmd(&env, Cmd::KeysAdd { note: "x".into() });
        let _ = std::fs::set_permissions(&env.data_path, {
            use std::os::unix::fs::PermissionsExt;
            std::fs::Permissions::from_mode(0o600)
        });
        assert_eq!(v["ok"], false, "{v}");
        assert_eq!(v["error"], "permission_denied", "{v}");
    }

    #[test]
    fn repl_help_then_quit() {
        let dir = tempfile::tempdir().unwrap();
        let env = test_env(dir.path());
        let mut out = Vec::new();
        run_repl(&env, std::io::Cursor::new("help\nquit\n"), &mut out).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains('>'), "{s}");
        assert!(s.contains("status"), "{s}");
    }

    #[test]
    fn json_lines_status_missing_socket() {
        let dir = tempfile::tempdir().unwrap();
        let env = test_env(dir.path());
        let mut out = Vec::new();
        run_json_lines(
            &env,
            std::io::Cursor::new("{\"cmd\":\"status\"}\n"),
            &mut out,
        )
        .unwrap();
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["ok"], false);
        assert_eq!(v["error"], "server_unreachable");
    }

    #[test]
    fn status_against_stub_uds_ok() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("ctl.sock");
        let sock2 = sock.clone();
        let h = std::thread::spawn(move || {
            let listener = std::os::unix::net::UnixListener::bind(&sock2).unwrap();
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = [0u8; 256];
            let _ = s.read(&mut buf);
            s.write_all(br#"{"state":"on"}"#).unwrap();
        });
        let mut env = test_env(dir.path());
        env.cfg.control_socket = sock;
        let mut v = json!(null);
        for _ in 0..40 {
            v = exec_cmd(&env, Cmd::Status);
            if v["ok"] == true {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = h.join();
        assert_eq!(v["ok"], true, "{v}");
        assert_eq!(v["control"]["state"], "on", "{v}");
    }

    #[test]
    fn install_link_temp_home() {
        let dir = tempfile::tempdir().unwrap();
        let tgt = dir.path().join("ctl-bin");
        std::fs::write(&tgt, b"x").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tgt, std::fs::Permissions::from_mode(0o755));
        }
        let home = dir.path().join("home");
        std::fs::create_dir(&home).unwrap();
        let link = install_link(&home, &tgt).unwrap();
        let meta = std::fs::symlink_metadata(&link).unwrap();
        assert!(meta.file_type().is_symlink());
    }
}
