//! Template: an **effectful** atom. Copy into your crate.
//!
//! Effectful atoms write the world: JSON files, process signals, tunnels.
//! Always check `ctx.allow_write`. Refuse with `AtomError::PureActuate` if false.
//!
//! Example: JSON pointer CRUD is already built in as `"json.crud"`:
//! `{ "path": "data.json", "op": "add"|"put"|"del", "pointer": "/keys/-", "value": … }`
//!
//! Domain: validated JSON command. Bound: 8 MiB file. Error: `AtomError`.

use std::path::Path;

use atomos::atom::AtomCtx;
use atomos::error::AtomError;
use serde_json::{json, Value};

pub fn run(ctx: &AtomCtx, input: Value) -> Result<Value, AtomError> {
    if !ctx.allow_write {
        return Err(AtomError::PureActuate);
    }
    let path = input
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AtomError::Input("path".into()))?;
    if Path::new(path).exists() {
        // replace this with the real effect
        return Ok(json!({ "ok": true, "path": path }));
    }
    Err(AtomError::NotFound)
}
