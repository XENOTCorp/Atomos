//! Template: an async API module. Input is `InOwned` (path/headers/body copied).
//!
//! Use this when the handler awaits (DB, upstream queue). CPU-heavy work still
//! belongs in `spawn_blocking` / rayon, not on the tokio worker.
//!
//! JSON in: `req.body` already passed the depth/size scan. Parse with serde.
//! JSON out: `atomos::json_out::to_bytes` (thread-local buffer).

use atomos::error::ServeError;
use atomos::io::{InOwned, Out};
use atomos::json_out;
use atomos::module::{AsyncModule, BoxFut};
use atomos::status::Status;

pub struct Api;

impl AsyncModule for Api {
    fn name(&self) -> &'static str {
        "api"
    }

    fn handle<'a>(&'a self, req: &'a InOwned) -> BoxFut<'a> {
        Box::pin(async move {
            match (req.method, req.path.as_str()) {
                (atomos::io::Method::Get, "/api/health") => {
                    let body = json_out::to_bytes(&serde_json::json!({ "ok": true }));
                    Ok(Out::json(Status::OK, body))
                }
                (atomos::io::Method::Post, "/api/echo") => {
                    // body is raw bytes; depth already checked if it looked like JSON
                    let v: serde_json::Value = serde_json::from_slice(&req.body)
                        .unwrap_or_else(|_| serde_json::json!({ "error": "json" }));
                    Ok(Out::json(Status::OK, json_out::to_bytes(&v)))
                }
                _ => Err(ServeError::NoRule),
            }
        })
    }
}
