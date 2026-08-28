//! Drop root, capabilities, and peer credential check.
use std::ffi::CString;
use std::os::fd::RawFd;
use crate::config::Config;
use crate::error::ServeError;

pub(crate) fn drop_privs(cfg: &Config) -> Result<(), ServeError> {
    let uid = unsafe { libc::geteuid() };
    if uid != 0 {
        return Ok(());
    }
    if let Some(ref gname) = cfg.drop_group {
        let gid = lookup_gid(gname)?;
        if unsafe { libc::setgid(gid) } != 0 {
            return Err(ServeError::Config("setgid failed".into()));
        }
    }
    if let Some(ref uname) = cfg.drop_user {
        let uid = lookup_uid(uname)?;
        if unsafe { libc::setuid(uid) } != 0 {
            return Err(ServeError::Config("setuid failed".into()));
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub(crate) fn lookup_uid(name: &str) -> Result<libc::uid_t, ServeError> {
    let c = CString::new(name).map_err(|_| ServeError::Config("drop_user".into()))?;
    let pw = unsafe { libc::getpwnam(c.as_ptr()) };
    if pw.is_null() {
        return Err(ServeError::Config("drop_user unknown".into()));
    }
    Ok(unsafe { (*pw).pw_uid })
}

#[cfg(target_os = "linux")]
pub(crate) fn lookup_gid(name: &str) -> Result<libc::gid_t, ServeError> {
    let c = CString::new(name).map_err(|_| ServeError::Config("drop_group".into()))?;
    let gr = unsafe { libc::getgrnam(c.as_ptr()) };
    if gr.is_null() {
        return Err(ServeError::Config("drop_group unknown".into()));
    }
    Ok(unsafe { (*gr).gr_gid })
}

/// Syscalls the H1 worker may issue. `execve` is not present (I_jail).
pub(crate) fn drop_remaining_caps() {
    for cap in 0..64u64 {
        let _ = unsafe { libc::prctl(libc::PR_CAPBSET_DROP, cap, 0, 0, 0) };
    }
}

/// x86_64 / asm-generic Landlock syscall numbers.
pub fn peer_euid_ok(fd: RawFd) -> bool {
    #[cfg(target_os = "linux")]
    {
        let mut ucred = libc::ucred {
            pid: 0,
            uid: 0,
            gid: 0,
        };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        let rc = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                (&mut ucred as *mut libc::ucred).cast(),
                &mut len,
            )
        };
        if rc != 0 {
            return false;
        }
        let me = unsafe { libc::geteuid() };
        ucred.uid == me || ucred.uid == 0
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = fd;
        true
    }
}

