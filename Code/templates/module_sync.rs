//! Template: a synchronous module (`In<'_>` borrows the receive buffer).
//!
//! `Out` fields you may set:
//! - `status`: `Status::OK`, `Status::NOT_FOUND`, …
//! - `reason`: None uses the RFC phrase
//! - `headers`: extra Content-Type, ETag, Location
//! - `body`: `OutBody::Raw` for files, `OutBody::Json` for APIs (already serialized)
//! - `cache`: `No` (default) | `Global { ttl_ms }` | `Named { ruleset, ttl_ms }`
//! - `flags`: passed to the post-module (`FLAG_LOG`, `FLAG_METRICS_SKIP`, `FLAG_NO_POST`)
//!
//! Register: `modules.insert("home", Handler::Sync(Arc::new(Home)));`
//! Bind it in `rules.json` with a disjoint include/exclude.
//!
//! ## Datapath notes for endpoint authors (guidance, not compiled)
//!
//! - **Allocators (jemalloc / mimalloc)** only reach the control path.
//!   The H1 datapath (atomos epoll on fds) preallocates at startup
//!   and allocates nothing per request, so swapping the allocator cannot
//!   speed up the hot loop: it changes control-path / connection-setup
//!   behavior, which matters for endpoints that build large responses.
//!   To opt in, in your **binary crate** (never in a module: one
//!   `#[global_allocator]` per process):
//!   ```rust,ignore
//!   // [dependencies] jemallocator = "0.5"   // or mimalloc = "0.1"
//!   #[global_allocator]
//!   static GLOBAL: jemallocator::Jemalloc = jemallocator::Jemalloc;
//!   ```
//! - **Lock-free handoff** between an endpoint and background workers:
//!   the kernel datapath is per-core run-to-completion (no locks, no
//!   channels: `src/net/epoll/`). When an endpoint must talk to
//!   another thread (fan-out, a writer pool), prefer a lock-free channel
//!   over `Mutex<Vec<...>>`:
//!   - `crossbeam-channel` (bounded) for one-shot jobs / queues;
//!   - atomics + a fixed ring for high-rate counters.
//!   Do NOT take a `Mutex` inside `handle()`: a blocked endpoint stalls
//!   the whole worker (run-to-completion). If a mutex is unavoidable,
//!   use `parking_lot` (faster than std, no poison).

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
