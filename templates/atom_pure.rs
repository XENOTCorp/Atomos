//! Template: a **pure** atom. Copy into your crate; do not perform I/O.
//!
//! Wire it in `atom::dispatch` as `"your.name" => your_fn(ctx, input)`.
//! Pure atoms may read `AtomCtx` (signal, rules snapshot, rss). They must not
//! write files, bind ports, or mutate `signal` / `stop`.
//!
//! Domain: JSON in, JSON out. Bound: keep output under 1 MiB.
//! Error: `AtomError::Input` / `Json` / `Bound`. Never `PureActuate` from here
//! (that error is for effectful atoms invoked without write permission).

use atomos::atom::AtomCtx;
use atomos::error::AtomError;
use serde_json::{json, Value};

pub fn run(ctx: &AtomCtx, input: Value) -> Result<Value, AtomError> {
    let _ = input;
    let rss = atomos::governor::Governor::rss_bytes();
    Ok(json!({
        "ok": true,
        "rss_bytes": rss,
        "uptime_ms": ctx.started.elapsed().as_millis() as u64
    }))
}
