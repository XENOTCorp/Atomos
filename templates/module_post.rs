//! Template: optional **post** module. Sees the module `Out` plus flags.
//!
//! The router currently calls `post.handle(&req)` (the original request).
//! Put extra headers or a rewritten body on the returned `Out`; non-empty body
//! and non-200 status replace the module result. Skip with `FLAG_NO_POST`.
//! Config: `"post_module": "post"`.

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
        let mut out = Out::empty(Status::from_u16(0));
        out.headers
            .push(("X-Atomos".into(), "1".into()));
        Ok(out)
    }
}
