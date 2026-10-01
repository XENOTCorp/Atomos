//! Immediate writes and bounded parking of response tails.
use super::conn::Conn;
use super::write::flush_out;
use super::{copy_into_out, out_max, tlsio};
use std::io;

/// Finish or park one response. A parked response is resumed by writable
/// readiness; callers must not dispatch another request or close the socket.
pub(super) fn finish_response(c: &mut Conn<'_>, keepalive: bool) -> io::Result<bool> {
    c.close_after_write = !keepalive;
    flush_out(c)?;
    Ok(c.close_after_write || c.has_pending_output())
}

fn park_out(c: &mut Conn<'_>, bytes: &[u8]) -> io::Result<()> {
    if copy_into_out(&mut c.out, bytes) {
        return Ok(());
    }
    let need = c.out.len().saturating_add(bytes.len());
    if need > out_max() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "epoll: out cap (ALLOC-01)",
        ));
    }
    c.out.reserve(bytes.len());
    c.out.extend_from_slice(bytes);
    Ok(())
}

pub(super) fn queue_bytes(c: &mut Conn<'_>, bytes: &[u8]) -> io::Result<()> {
    // Write first when `out` is empty (cached GET: one send, no extra
    // copy). Park into `out` only for a leftover or a queued tail.
    if !c.out.is_empty() {
        if copy_into_out(&mut c.out, bytes) {
            return Ok(());
        }
        flush_out(c)?;
        if copy_into_out(&mut c.out, bytes) {
            return Ok(());
        }
        if !c.out.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "epoll: out cap (ALLOC-01)",
            ));
        }
    }
    if c.tls.is_none() {
        match c.stream.writev(&[bytes]) {
            Ok(n) if n == bytes.len() => Ok(()),
            Ok(n) => park_out(c, &bytes[n..]),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => park_out(c, bytes),
            Err(e) => Err(e),
        }
    } else {
        match tlsio::write_plain(c, bytes) {
            Ok(n) if n == bytes.len() => Ok(()),
            Ok(n) => park_out(c, &bytes[n..]),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => park_out(c, bytes),
            Err(e) => Err(e),
        }
    }
}
