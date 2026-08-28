//! Flush encoded bytes and sendfile.
use std::io;
use std::os::fd::AsRawFd;
use super::conn::Conn;

pub(crate) fn flush_out(c: &mut Conn<'_>) -> io::Result<()> {
    while c.out_off < c.out.len() {
        let rest = &c.out[c.out_off..];
        // Same syscall as FDS `write_all` (`send` + MSG_NOSIGNAL), but
        // the offset is kept so a partial write does not drop the tail.
        let n = unsafe {
            libc::send(
                c.stream.as_raw_fd(),
                rest.as_ptr() as *const libc::c_void,
                rest.len(),
                libc::MSG_NOSIGNAL,
            )
        };
        if n < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::WouldBlock {
                return Ok(());
            }
            return Err(e);
        }
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "epoll: send zero",
            ));
        }
        c.out_off += n as usize;
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
