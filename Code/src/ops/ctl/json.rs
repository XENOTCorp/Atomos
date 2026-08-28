//! JSON line operator API.
use std::io::{BufRead, Write};
use std::path::Path;
use serde_json::{json, Value};
use crate::error::AtomError;
use super::{Cmd, Env, exec_cmd};
use super::cmd::{parse_json, parse_line};

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

pub(crate) fn connect_error_json(err: &std::io::Error, path: &Path) -> Value {
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
