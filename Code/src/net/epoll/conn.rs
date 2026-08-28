//! Per-connection HTTP/1.1 state on an FDS table slot.
use std::net::SocketAddr;
use std::sync::Arc;
use fds::conn::{ConnectionSlot, CONN_CAP};
use fds::tcp::TcpStream;

pub(crate) struct Conn<'a> {
    pub(crate) stream: TcpStream,
    pub(crate) peer: SocketAddr,
    pub(crate) buf: Vec<u8>,
    /// Bytes of `buf` already consumed by completed requests (read
    /// cursor; compacted out past [`COMPACT_THRESHOLD`]).
    pub(crate) pos: usize,
    pub(crate) out: Vec<u8>,
    pub(crate) out_off: usize,
    /// File body being sent with `sendfile` after the header bytes in
    /// `out` are flushed. Non-None means the response is not fully
    /// written: the worker keeps writable interest and skips reads
    /// until it completes (request/response order must be preserved).
    pub(crate) pending_sf: Option<PendingSf>,
    /// Held for the connection's lifetime: dropping it releases the
    /// table slot exactly once (never call `release_slot` while a guard
    /// is alive — that would double-release the free-list ring).
    pub(crate) slot: ConnectionSlot<'a, CONN_CAP>,
}

/// Remaining range of an open file to send kernel-side.
pub(crate) struct PendingSf {
    pub(crate) file: Arc<std::fs::File>,
    /// `off_t` so it can be passed to `sendfile` without a cast.
    pub(crate) offset: libc::off_t,
    pub(crate) len: u64,
}

