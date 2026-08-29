//! Per-connection HTTP/1.1 state on an FDS table slot.
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;
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
    /// Last successful read or write. Body and idle timeouts use this.
    pub(crate) last_rw: Instant,
    /// First byte of the current header block. Slowloris uses this
    /// clock (not `last_rw`) so a 1-byte/2s drip still expires.
    pub(crate) hdr_t0: Option<Instant>,
    /// At least one request has been fully served on this slot.
    pub(crate) served: bool,
    /// rustls server state when `h1_tls`. Handshake and app data share
    /// the FDS fd via `read_tls`/`write_tls`.
    pub(crate) tls: Option<Box<rustls::ServerConnection>>,
    /// Held for the connection's lifetime: dropping it releases the
    /// table slot exactly once (never call `release_slot` while a guard
    /// is alive: that would double-release the free-list ring).
    pub(crate) slot: ConnectionSlot<'a, CONN_CAP>,
}

/// Remaining range of an open file to send kernel-side.
pub(crate) struct PendingSf {
    pub(crate) file: Arc<std::fs::File>,
    /// `off_t` so it can be passed to `sendfile` without a cast.
    pub(crate) offset: libc::off_t,
    pub(crate) len: u64,
}

