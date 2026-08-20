//! Template: a synchronous module (`In<'_>` borrows the receive buffer).
//!
//! `Out` fields you may set:
//! - `status` — `Status::OK`, `Status::NOT_FOUND`, …
//! - `reason` — None uses the RFC phrase
//! - `headers` — extra Content-Type, ETag, Location
//! - `body` — `OutBody::Raw` for files, `OutBody::Json` for APIs (already serialized)
//! - `cache` — `No` (default) | `Global { ttl_ms }` | `Named { ruleset, ttl_ms }`
//! - `flags` — passed to the post-module (`FLAG_LOG`, `FLAG_METRICS_SKIP`, `FLAG_NO_POST`)
//!
//! Register: `modules.insert("home", Handler::Sync(Arc::new(Home)));`
//! Bind it in `rules.json` with a disjoint include/exclude.

use atomos::error::ServeError;
use atomos::io::{CacheDirective, In, Out, OutBody};
use atomos::module::Module;
use atomos::status::Status;
use bytes::Bytes;

pub struct Home;

impl Module for Home {
    fn name(&self) -> &'static str {
        "home"
    }

    fn handle(&self, req: &In<'_>) -> Result<Out, ServeError> {
        if req.path != "/" {
            return Err(ServeError::NoRule);
        }
        Ok(Out {
            status: Status::OK,
            reason: None,
            headers: vec![("Content-Type".into(), "text/plain; charset=utf-8".into())],
            body: OutBody::Raw(Bytes::from_static(b"ok\n")),
            cache: CacheDirective::Global { ttl_ms: 5_000 },
            flags: atomos::flags::FlagSet::empty(),
        })
    }
}
