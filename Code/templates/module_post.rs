//! Template: optional **post** module. Sees the module `Out` plus flags.
//!
//! The router currently calls `post.handle(&req)` (the original request).
//! Status 0 skips merge entirely. Return 200 + extra headers to append
//! headers; non-empty body replaces the module body; non-200 replaces status.
//! Skip with `FLAG_NO_POST`. Config: `"post_module": "post"` (you still set
//! `router.post`).
//!
//! ## Datapath notes (guidance, not compiled)
//!
//! - Keep this module allocation-free per call (it runs on the same
//!   per-core run-to-completion loop as the datapath). jemalloc/mimalloc
//!   opt-in and the lock-free handoff patterns live in `module_sync.rs`.
//! - If the post-module feeds observability (logging, metrics fan-out),
//!   push through a bounded lock-free channel (`crossbeam-channel`) or
//!   atomics; never block the worker on a mutex.

use atomos::error::ServeError;
use atomos::io::{In, Out};
use atomos::module::Module;
use atomos::status::Status;

pub struct Post;

impl Module for Post {
    fn name(&self) -> &'static str {
        "post"
    }

    fn handle(&self, _req: &In<'_>) -> Result<Out, ServeError> {
        let mut out = Out::empty(Status::OK);
        out.headers
            .push(("X-Atomos".into(), "1".into()));
        Ok(out)
    }
}
