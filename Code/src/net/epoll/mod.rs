//! FDS-backed HTTP/1.1 engine. The server owns readiness and connection
//! lifecycle; request handling, bounded buffers, queuing, TLS and file writes
//! are separate implementation modules. No task or hash lookup per connection.
//!
//! Cached wire lookup/encoding is allocation-free after warm-up. Parsing still
//! owns a header Vec, and periodic RSS sampling is outside that guarantee.

mod buffer;
mod conn;
mod queue;
mod request;
mod server;
#[cfg(test)]
mod tests;
mod tlsio;
mod write;

use crate::static_mod::SF_MIN;
use fds::conn::{ConnectionId, CONN_CAP};
pub use server::run;

const TOKEN_LISTENER: u64 = u64::MAX;
const COMPACT_THRESHOLD: usize = 64 * 1024;

/// Initial request-buffer capacity. Consumed space is reclaimed before growth.
pub const IN_CAP: usize = 4096;
/// Initial response/header scratch capacity.
pub const OUT_CAP: usize = 2048;

pub const fn out_max() -> usize {
    SF_MIN as usize + OUT_CAP
}

/// Decode the slot portion; the server additionally checks the full token's
/// generation so readiness from a recycled slot cannot target its new owner.
pub const fn http_slot(token: u64) -> Option<usize> {
    if token == TOKEN_LISTENER {
        return None;
    }
    let slot = ConnectionId::from_u64(token).slot() as usize;
    if slot < CONN_CAP {
        Some(slot)
    } else {
        None
    }
}

pub const fn buf_capacity_for(max_header: usize, max_body: usize) -> usize {
    max_header.saturating_add(max_body)
}

pub fn append_in_cap(buf: &mut Vec<u8>, src: &[u8]) -> bool {
    if src.len() > buf.capacity().saturating_sub(buf.len()) {
        return false;
    }
    buf.extend_from_slice(src);
    true
}

pub fn copy_into_out(out: &mut Vec<u8>, src: &[u8]) -> bool {
    append_in_cap(out, src)
}
