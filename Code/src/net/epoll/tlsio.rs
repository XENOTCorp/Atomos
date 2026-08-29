//! rustls `ServerConnection` on an FDS TCP fd. ALPN http/1.1 only.

use std::io::{self, Read, Write};

use fds::tcp::TcpStream;
use rustls::ServerConnection;

use super::conn::Conn;

pub(crate) struct FdIo<'a>(pub(crate) &'a mut TcpStream);

impl Read for FdIo<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

impl Write for FdIo<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = unsafe {
            libc::send(
                self.0.as_raw_fd(),
                buf.as_ptr() as *const libc::c_void,
                buf.len(),
                libc::MSG_NOSIGNAL,
            )
        };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(n as usize)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn tls_io(e: rustls::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e)
}

pub(crate) fn alpn_is_h1(tls: &ServerConnection) -> bool {
    match tls.alpn_protocol() {
        None => true,
        Some(p) => p == b"http/1.1",
    }
}

pub(crate) fn wants_write(c: &Conn<'_>) -> bool {
    c.tls.as_ref().is_some_and(|t| t.wants_write())
}

pub(crate) fn flush_tls(c: &mut Conn<'_>) -> io::Result<()> {
    let Some(tls) = c.tls.as_mut() else {
        return Ok(());
    };
    while tls.wants_write() {
        match tls.write_tls(&mut FdIo(&mut c.stream)) {
            Ok(0) => break,
            Ok(_) => c.last_rw = std::time::Instant::now(),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(()),
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Pump TLS records. `Ok(None)` = wait. `Ok(Some(0))` = EOF after
/// handshake. `Ok(Some(n))` = plaintext bytes in `tmp`. Handshake
/// incomplete returns `Ok(None)` (not EOF).
pub(crate) fn read_plain(c: &mut Conn<'_>, tmp: &mut [u8]) -> io::Result<Option<usize>> {
    let Some(tls) = c.tls.as_mut() else {
        return match c.stream.read(tmp) {
            Ok(n) => Ok(Some(n)),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(None),
            Err(e) => Err(e),
        };
    };
    loop {
        match tls.read_tls(&mut FdIo(&mut c.stream)) {
            Ok(0) => {
                if tls.is_handshaking() {
                    return Ok(Some(0));
                }
                break;
            }
            Ok(_) => {
                tls.process_new_packets().map_err(tls_io)?;
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
            Err(e) => return Err(e),
        }
    }
    if tls.wants_write() {
        flush_tls(c)?;
    }
    let tls = c.tls.as_mut().expect("tls");
    if tls.is_handshaking() {
        return Ok(None);
    }
    if !alpn_is_h1(tls) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "epoll: ALPN not http/1.1",
        ));
    }
    match tls.reader().read(tmp) {
        Ok(0) => Ok(Some(0)),
        Ok(n) => Ok(Some(n)),
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(None),
        Err(e) => Err(e),
    }
}

pub(crate) fn write_plain(c: &mut Conn<'_>, bytes: &[u8]) -> io::Result<usize> {
    let Some(tls) = c.tls.as_mut() else {
        return c.stream.writev(&[bytes]);
    };
    if tls.is_handshaking() {
        flush_tls(c)?;
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "epoll: tls handshake",
        ));
    }
    let n = tls.writer().write(bytes)?;
    flush_tls(c)?;
    Ok(n)
}
