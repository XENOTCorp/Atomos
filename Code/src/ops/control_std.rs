//! Blocking Unix-domain JSON ctl. H1 process. Same schema as `control.rs`.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::sync::Arc;

use serde::Deserialize;

use crate::atom::{dispatch, AtomCtx};
use crate::error::ServeError;
use crate::jail;

#[derive(Deserialize)]
struct Cmd {
    cmd: String,
}

pub fn serve_control(path: std::path::PathBuf, ctx: Arc<AtomCtx>) -> Result<(), ServeError> {
    jail::prepare_socket_dir(&path)?;
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    listener.set_nonblocking(false)?;
    loop {
        if ctx.stop.v.load(std::sync::atomic::Ordering::Acquire) != 0 {
            break;
        }
        let (sock, _) = listener.accept()?;
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            if !jail::peer_euid_ok(sock.as_raw_fd()) {
                continue;
            }
        }
        let mut reader = BufReader::new(sock);
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            continue;
        }
        let v = match serde_json::from_str::<Cmd>(&line) {
            Ok(c) => handle_cmd(&ctx, &c.cmd),
            Err(e) => serde_json::json!({"ok": false, "error": e.to_string()}),
        };
        let mut sock = reader.into_inner();
        let out = serde_json::to_vec(&v).unwrap_or_else(|_| b"{}".to_vec());
        let _ = sock.write_all(&out);
        let _ = sock.write_all(b"\n");
    }
    Ok(())
}

fn handle_cmd(ctx: &AtomCtx, cmd: &str) -> serde_json::Value {
    let name = match cmd {
        "status" => "signal.get",
        "refresh-endpoints" | "rules.reload" => "rules.reload",
        "cache.purge" | "purge" => "cache.purge",
        "stop" => "server.stop",
        "start" => "server.start",
        "restart" => "server.restart",
        "dry-test-rules" => "rules.dry_test",
        _ => {
            return serde_json::json!({"ok": false, "error": "unknown cmd"});
        }
    };
    match dispatch(ctx, name, serde_json::json!({})) {
        Ok(v) => v,
        Err(e) => serde_json::json!({"ok": false, "error": e.to_string()}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_cmd_is_json_error() {
        let ctx = AtomCtx::test();
        let v = handle_cmd(&ctx, "nope");
        assert_eq!(v["ok"], false);
    }
}
