//! Flush encoded bytes and sendfile. TLS encrypts; no sendfile through rustls.
use std::io;
use std::os::fd::AsRawFd;
use super::conn::Conn;
use super::tlsio;

pub(crate) fn flush_out(c: &mut Conn<'_>) -> io::Result<()> {
    if c.tls.is_some() {
        return flush_out_tls(c);
    }
    while c.out_off < c.out.len() {
        let rest = &c.out[c.out_off..];
        // FDS `TcpStream::write` (`send` + MSG_NOSIGNAL). Offset is kept
        // so a partial write does not drop the tail.
        let n = match c.stream.write(rest) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "epoll: send zero",
                ));
            }
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(()),
            Err(e) => return Err(e),
        };
        c.out_off += n;
        c.last_rw = std::time::Instant::now();
    }
    c.out.clear();
    c.out_off = 0;
    // Drain the file body kernel-side. Take/put so the borrow checker
    // does not hold `pending_sf` while `stream` is borrowed.
    loop {
        let mut sf = match c.pending_sf.take() {
            Some(s) => s,
            None => break,
        };
        // SAFETY: both fds are valid and owned by this connection; the
        // offset/count stay within the file range the fd was opened
        // for (StaticMod sets offset=0, len=file size).
        let n = unsafe {
            libc::sendfile(
                c.stream.as_raw_fd(),
                sf.file.as_raw_fd(),
                &mut sf.offset as *mut libc::off_t,
                sf.len as usize,
            )
        };
        if n < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::WouldBlock {
                c.pending_sf = Some(sf);
                return Ok(());
            }
            return Err(e);
        }
        let n = n as u64;
        if n == 0 {
            // EOF before the range was sent: the file shrank underneath
            // us (cache is stale). Bail out rather than hang.
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "sendfile: file shorter than cached range",
            ));
        }
        c.last_rw = std::time::Instant::now();
        if n >= sf.len {
            // Range fully sent.
            continue;
        }
        sf.offset += n as libc::off_t;
        sf.len -= n;
        c.pending_sf = Some(sf);
    }
    Ok(())
}

fn flush_out_tls(c: &mut Conn<'_>) -> io::Result<()> {
    let mut tmp = [0u8; 4096];
    while c.out_off < c.out.len() {
        let n = (c.out.len() - c.out_off).min(tmp.len());
        tmp[..n].copy_from_slice(&c.out[c.out_off..c.out_off + n]);
        match tlsio::write_plain(c, &tmp[..n]) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "epoll: tls write zero",
                ));
            }
            Ok(n) => {
                c.out_off += n;
                c.last_rw = std::time::Instant::now();
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(()),
            Err(e) => return Err(e),
        }
    }
    c.out.clear();
    c.out_off = 0;
    let mut tmp = [0u8; 16 * 1024];
    loop {
        let mut sf = match c.pending_sf.take() {
            Some(s) => s,
            None => break,
        };
        let want = (sf.len as usize).min(tmp.len());
        let n = {
            let n = unsafe {
                libc::pread(
                    sf.file.as_raw_fd(),
                    tmp.as_mut_ptr() as *mut libc::c_void,
                    want,
                    sf.offset,
                )
            };
            if n < 0 {
                let e = io::Error::last_os_error();
                if e.kind() == io::ErrorKind::WouldBlock {
                    c.pending_sf = Some(sf);
                    return Ok(());
                }
                return Err(e);
            }
            n as usize
        };
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "tls sendfile: file shorter than range",
            ));
        }
        match tlsio::write_plain(c, &tmp[..n]) {
            Ok(0) => {
                c.pending_sf = Some(sf);
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "epoll: tls file write zero",
                ));
            }
            Ok(w) => {
                sf.offset += w as libc::off_t;
                sf.len -= w as u64;
                c.last_rw = std::time::Instant::now();
                if sf.len > 0 {
                    c.pending_sf = Some(sf);
                    if w < n {
                        return Ok(());
                    }
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                c.pending_sf = Some(sf);
                return Ok(());
            }
            Err(e) => return Err(e),
        }
    }
    tlsio::flush_tls(c)
}
