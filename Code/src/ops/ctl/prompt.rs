//! Interactive prompt and human output.
use std::io::{BufRead, Write};
use serde_json::Value;
use super::{Cmd, Env, exec_cmd};
use super::cmd::parse_line;

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
