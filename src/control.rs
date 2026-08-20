//! Unix-domain JSON commands. Mode 0600. Criticality C2.

use std::sync::Arc;

use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

use crate::atom::{dispatch, AtomCtx};
use crate::error::ServeError;

#[derive(Deserialize)]
struct Cmd {
    cmd: String,
}

pub async fn serve_control(path: std::path::PathBuf, ctx: Arc<AtomCtx>) -> Result<(), ServeError> {
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    loop {
        if ctx.stop.v.load(std::sync::atomic::Ordering::Acquire) != 0 {
            break;
        }
        let (sock, _) = listener.accept().await?;
        let ctx = ctx.clone();
        tokio::spawn(async move {
            let mut sock = sock;
            let mut reader = BufReader::new(&mut sock);
            let mut line = String::new();
            if reader.read_line(&mut line).await.ok().unwrap_or(0) == 0 {
                return;
            }
            let v = match serde_json::from_str::<Cmd>(&line) {
                Ok(c) => handle_cmd(&ctx, &c.cmd),
                Err(e) => serde_json::json!({"ok": false, "error": e.to_string()}),
            };
            let out = serde_json::to_vec(&v).unwrap_or_else(|_| b"{}".to_vec());
            let _ = sock.write_all(&out).await;
            let _ = sock.write_all(b"\n").await;
        });
    }
    Ok(())
}

fn handle_cmd(ctx: &AtomCtx, cmd: &str) -> serde_json::Value {
    let name = match cmd {
        "status" => "signal.get",
        "refresh-endpoints" | "rules.reload" => "rules.reload",
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
