//! Template: optional **pre** module. Runs after accept/parse, before ruleset.
//!
//! Use for firewall / auth / scheduling. Return status ≥ 400 to short-circuit.
//! Otherwise set `req.flags` via the returned `Out.flags` (the router copies
//! them onto the request). Keep this RAM-resident; name it in config
//! `"pre_module": "pre"`.
//!
//! ## Datapath notes (guidance, not compiled)
//!
//! - The H1 datapath allocates nothing per request; keep this module the
//!   same (borrow `req`, avoid `String`/`Vec` per call). jemalloc /
//!   mimalloc swaps only affect the control path: see `module_sync.rs`
//!   for the `#[global_allocator]` pattern.
//! - Lock-free: if you gate on a shared rate-limiter / token bucket, use
//!   atomics or `crossbeam-channel`; a `parking_lot` mutex is acceptable
//!   on the control path but never inside `handle()` (run-to-completion:
//!   a blocked pre-module stalls the worker).

use atomos::error::ServeError;
use atomos::flags::{FlagSet, FLAG_LOG};
use atomos::io::{In, Out};
use atomos::module::Module;
use atomos::status::Status;

pub struct Pre;

impl Module for Pre {
    fn name(&self) -> &'static str {
        "pre"
    }

    fn handle(&self, req: &In<'_>) -> Result<Out, ServeError> {
        if !req.peer.ip().is_loopback() {
            return Ok(Out::empty(Status::FORBIDDEN));
        }
        let mut out = Out::empty(Status::OK);
        let mut flags = FlagSet::empty();
        flags.insert(FLAG_LOG);
        out.flags = flags;
        Ok(out)
    }
}
