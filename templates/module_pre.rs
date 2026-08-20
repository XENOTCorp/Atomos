//! Template: optional **pre** module. Runs after accept/parse, before ruleset.
//!
//! Use for firewall / auth / scheduling. Return status ≥ 400 to short-circuit.
//! Otherwise set `req.flags` via the returned `Out.flags` (the router copies
//! them onto the request). Keep this RAM-resident; name it in config
//! `"pre_module": "pre"`.

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
