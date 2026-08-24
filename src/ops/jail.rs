//! Post-bind jail: drop root, no_new_privs, SO_PEERCRED on ctl. Linux.

use std::ffi::CString;
use std::os::fd::RawFd;
use std::path::Path;

use crate::config::Config;
use crate::error::ServeError;

/// Ensure parent dir exists (0700) for the control socket.
pub fn prepare_socket_dir(sock: &Path) -> Result<(), ServeError> {
    if let Some(dir) = sock.parent() {
        if dir.as_os_str().is_empty() {
            return Ok(());
        }
        if !dir.exists() {
            std::fs::create_dir_all(dir)?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        }
    }
    Ok(())
}

/// After listeners are bound. No-op if not root / not Linux.
pub fn after_bind(cfg: &Config) -> Result<(), ServeError> {
    #[cfg(target_os = "linux")]
    {
        if cfg.no_new_privs {
            // SAFETY: PR_SET_NO_NEW_PRIVS is a process-wide flag; 1, 0, 0 are the documented args.
            let rc = unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) };
            if rc != 0 {
                tracing::warn!("prctl NO_NEW_PRIVS failed");
            }
        }
        drop_privs(cfg)?;
        if cfg.drop_caps {
            drop_remaining_caps();
        }
        if cfg.landlock {
            landlock_restrict(cfg)?;
        }
        if cfg.seccomp {
            seccomp_allowlist()?;
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = cfg;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn drop_privs(cfg: &Config) -> Result<(), ServeError> {
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
fn lookup_uid(name: &str) -> Result<libc::uid_t, ServeError> {
    let c = CString::new(name).map_err(|_| ServeError::Config("drop_user".into()))?;
    let pw = unsafe { libc::getpwnam(c.as_ptr()) };
    if pw.is_null() {
        return Err(ServeError::Config("drop_user unknown".into()));
    }
    Ok(unsafe { (*pw).pw_uid })
}

#[cfg(target_os = "linux")]
fn lookup_gid(name: &str) -> Result<libc::gid_t, ServeError> {
    let c = CString::new(name).map_err(|_| ServeError::Config("drop_group".into()))?;
    let gr = unsafe { libc::getgrnam(c.as_ptr()) };
    if gr.is_null() {
        return Err(ServeError::Config("drop_group unknown".into()));
    }
    Ok(unsafe { (*gr).gr_gid })
}

/// Syscalls the H1 worker may issue. `execve` is not present (I_jail).
#[cfg(target_os = "linux")]
pub const SECCOMP_ALLOW: &[i64] = &[
    libc::SYS_read,
    libc::SYS_write,
    libc::SYS_close,
    libc::SYS_epoll_wait,
    libc::SYS_epoll_pwait,
    libc::SYS_epoll_ctl,
    libc::SYS_epoll_create1,
    libc::SYS_accept,
    libc::SYS_accept4,
    libc::SYS_recvfrom,
    libc::SYS_sendto,
    libc::SYS_mmap,
    libc::SYS_mprotect,
    libc::SYS_brk,
    libc::SYS_clone,
    libc::SYS_futex,
    libc::SYS_nanosleep,
    libc::SYS_exit_group,
    libc::SYS_rt_sigreturn,
    libc::SYS_pipe2,
    libc::SYS_writev,
    libc::SYS_clock_gettime,
    libc::SYS_getrandom,
    // static open + thread/runtime glue for epoll workers
    libc::SYS_openat,
    libc::SYS_newfstatat,
    libc::SYS_fstat,
    libc::SYS_munmap,
    libc::SYS_madvise,
    libc::SYS_rt_sigaction,
    libc::SYS_rt_sigprocmask,
    libc::SYS_exit,
    libc::SYS_socket,
    libc::SYS_bind,
    libc::SYS_listen,
    libc::SYS_setsockopt,
    libc::SYS_getsockopt,
    libc::SYS_shutdown,
    libc::SYS_fcntl,
    libc::SYS_ioctl,
    libc::SYS_lseek,
    libc::SYS_pread64,
    libc::SYS_pwrite64,
    libc::SYS_getpid,
    libc::SYS_gettid,
    libc::SYS_sched_yield,
    libc::SYS_sched_getaffinity,
    libc::SYS_set_robust_list,
    libc::SYS_prctl,
    libc::SYS_recvmsg,
    libc::SYS_sendmsg,
];

#[cfg(target_os = "linux")]
fn drop_remaining_caps() {
    for cap in 0..64u64 {
        let _ = unsafe { libc::prctl(libc::PR_CAPBSET_DROP, cap, 0, 0, 0) };
    }
}

/// x86_64 / asm-generic Landlock syscall numbers.
#[cfg(target_os = "linux")]
const SYS_LANDLOCK_CREATE_RULESET: libc::c_long = 444;
#[cfg(target_os = "linux")]
const SYS_LANDLOCK_ADD_RULE: libc::c_long = 445;
#[cfg(target_os = "linux")]
const SYS_LANDLOCK_RESTRICT_SELF: libc::c_long = 446;

#[cfg(target_os = "linux")]
const LANDLOCK_ACCESS_FS_WRITE_FILE: u64 = 1 << 1;
#[cfg(target_os = "linux")]
const LANDLOCK_ACCESS_FS_READ_FILE: u64 = 1 << 2;
#[cfg(target_os = "linux")]
const LANDLOCK_ACCESS_FS_READ_DIR: u64 = 1 << 3;
#[cfg(target_os = "linux")]
const LANDLOCK_RULE_PATH_BENEATH: i32 = 1;

#[cfg(target_os = "linux")]
#[repr(C)]
struct LandlockRulesetAttr {
    handled_access_fs: u64,
}

#[cfg(target_os = "linux")]
#[repr(C, packed)]
struct LandlockPathBeneathAttr {
    allowed_access: u64,
    parent_fd: i32,
}

#[cfg(target_os = "linux")]
fn landlock_unsupported(err: i32) -> bool {
    err == libc::ENOSYS || err == libc::EOPNOTSUPP
}

#[cfg(target_os = "linux")]
fn landlock_errno() -> i32 {
    unsafe { *libc::__errno_location() }
}

#[cfg(target_os = "linux")]
fn path_parent_for_rule(path: &Path) -> Option<&Path> {
    match path.parent() {
        None => None,
        Some(p) if p.as_os_str().is_empty() => Some(Path::new(".")),
        Some(p) => Some(p),
    }
}

#[cfg(target_os = "linux")]
fn landlock_add_path(ruleset_fd: libc::c_int, path: &Path, allowed: u64) -> Result<(), ServeError> {
    if !path.exists() {
        tracing::warn!(path = %path.display(), "landlock: path missing, skip rule");
        return Ok(());
    }
    let c = CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|_| ServeError::Config("landlock path".into()))?;
    let pfd = unsafe { libc::open(c.as_ptr(), libc::O_PATH | libc::O_CLOEXEC) };
    if pfd < 0 {
        tracing::warn!(path = %path.display(), "landlock: open O_PATH failed, skip rule");
        return Ok(());
    }
    let attr = LandlockPathBeneathAttr {
        allowed_access: allowed,
        parent_fd: pfd,
    };
    let rc = unsafe {
        libc::syscall(
            SYS_LANDLOCK_ADD_RULE,
            ruleset_fd,
            LANDLOCK_RULE_PATH_BENEATH,
            &attr as *const LandlockPathBeneathAttr,
            0i32,
        )
    };
    unsafe {
        libc::close(pfd);
    }
    if rc != 0 {
        let err = landlock_errno();
        if landlock_unsupported(err) {
            tracing::warn!(errno = err, "landlock_add_rule unsupported");
            return Ok(());
        }
        return Err(ServeError::Config(
            format!("landlock_add_rule failed errno={err}").into(),
        ));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn landlock_restrict(cfg: &Config) -> Result<(), ServeError> {
    let handled =
        LANDLOCK_ACCESS_FS_READ_FILE | LANDLOCK_ACCESS_FS_READ_DIR | LANDLOCK_ACCESS_FS_WRITE_FILE;
    let attr = LandlockRulesetAttr {
        handled_access_fs: handled,
    };
    let ruleset_fd = unsafe {
        libc::syscall(
            SYS_LANDLOCK_CREATE_RULESET,
            &attr as *const LandlockRulesetAttr,
            std::mem::size_of::<LandlockRulesetAttr>(),
            0u32,
        )
    };
    if ruleset_fd < 0 {
        let err = landlock_errno();
        if landlock_unsupported(err) {
            tracing::warn!(errno = err, "landlock_create_ruleset unsupported");
            return Ok(());
        }
        return Err(ServeError::Config(
            format!("landlock_create_ruleset failed errno={err}").into(),
        ));
    }
    let ruleset_fd = ruleset_fd as libc::c_int;

    let read_only = LANDLOCK_ACCESS_FS_READ_FILE | LANDLOCK_ACCESS_FS_READ_DIR;
    let read_write = read_only | LANDLOCK_ACCESS_FS_WRITE_FILE;

    let result = (|| {
        landlock_add_path(ruleset_fd, &cfg.static_root, read_only)?;
        if let Some(p) = path_parent_for_rule(&cfg.rules_path) {
            landlock_add_path(ruleset_fd, p, read_write)?;
        }
        if let Some(p) = path_parent_for_rule(&cfg.control_socket) {
            landlock_add_path(ruleset_fd, p, read_write)?;
        }
        let rc = unsafe { libc::syscall(SYS_LANDLOCK_RESTRICT_SELF, ruleset_fd, 0u32) };
        if rc != 0 {
            let err = landlock_errno();
            if landlock_unsupported(err) {
                tracing::warn!(errno = err, "landlock_restrict_self unsupported");
                return Ok(());
            }
            return Err(ServeError::Config(
                format!("landlock_restrict_self failed errno={err}").into(),
            ));
        }
        tracing::info!(
            static_root = %cfg.static_root.display(),
            "landlock restrict_self applied"
        );
        Ok(())
    })();

    unsafe {
        libc::close(ruleset_fd);
    }
    result
}

/// Classic BPF filter bytes for `SECCOMP_ALLOW` (x86_64 arch check + allowlist).
#[cfg(target_os = "linux")]
pub fn seccomp_filter_bytes() -> Vec<u8> {
    let prog = seccomp_filter_prog();
    let mut out = Vec::with_capacity(prog.len() * std::mem::size_of::<libc::sock_filter>());
    for insn in prog {
        out.extend_from_slice(&insn.code.to_ne_bytes());
        out.push(insn.jt);
        out.push(insn.jf);
        out.extend_from_slice(&insn.k.to_ne_bytes());
    }
    out
}

#[cfg(target_os = "linux")]
fn bpf_stmt(code: u32, k: u32) -> libc::sock_filter {
    libc::sock_filter {
        code: code as u16,
        jt: 0,
        jf: 0,
        k,
    }
}

#[cfg(target_os = "linux")]
fn bpf_jump(code: u32, k: u32, jt: u8, jf: u8) -> libc::sock_filter {
    libc::sock_filter {
        code: code as u16,
        jt,
        jf,
        k,
    }
}

/// AUDIT_ARCH_X86_64 — not always exported by libc.
#[cfg(target_os = "linux")]
const AUDIT_ARCH_X86_64: u32 = 0xc000_003e;

#[cfg(target_os = "linux")]
fn seccomp_filter_prog() -> Vec<libc::sock_filter> {
    // Validate arch, then allow listed nr, else KILL_THREAD.
    let mut f = Vec::with_capacity(4 + SECCOMP_ALLOW.len() * 2);
    f.push(bpf_stmt(
        libc::BPF_LD | libc::BPF_W | libc::BPF_ABS,
        4, // offsetof(seccomp_data, arch)
    ));
    f.push(bpf_jump(
        libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K,
        AUDIT_ARCH_X86_64,
        1,
        0,
    ));
    f.push(bpf_stmt(
        libc::BPF_RET | libc::BPF_K,
        libc::SECCOMP_RET_KILL_THREAD,
    ));
    f.push(bpf_stmt(
        libc::BPF_LD | libc::BPF_W | libc::BPF_ABS,
        0, // offsetof(seccomp_data, nr)
    ));
    for &nr in SECCOMP_ALLOW {
        // match → fall through to ALLOW; else skip ALLOW
        f.push(bpf_jump(
            libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K,
            nr as u32,
            0,
            1,
        ));
        f.push(bpf_stmt(
            libc::BPF_RET | libc::BPF_K,
            libc::SECCOMP_RET_ALLOW,
        ));
    }
    f.push(bpf_stmt(
        libc::BPF_RET | libc::BPF_K,
        libc::SECCOMP_RET_KILL_THREAD,
    ));
    f
}

#[cfg(target_os = "linux")]
fn seccomp_allowlist() -> Result<(), ServeError> {
    if SECCOMP_ALLOW.contains(&libc::SYS_execve) {
        return Err(ServeError::Config(
            "seccomp allowlist contains execve".into(),
        ));
    }
    let mut filter = seccomp_filter_prog();
    if filter.is_empty() {
        return Err(ServeError::Config("seccomp filter empty".into()));
    }
    let prog = libc::sock_fprog {
        len: filter.len() as libc::c_ushort,
        filter: filter.as_mut_ptr(),
    };
    // Prefer seccomp(2); fall back to prctl(PR_SET_SECCOMP).
    let rc = unsafe {
        libc::syscall(
            libc::SYS_seccomp,
            libc::SECCOMP_SET_MODE_FILTER,
            0u32,
            &prog as *const libc::sock_fprog,
        )
    };
    if rc != 0 {
        let rc2 = unsafe {
            libc::prctl(
                libc::PR_SET_SECCOMP,
                libc::SECCOMP_MODE_FILTER as libc::c_ulong,
                &prog as *const libc::sock_fprog as libc::c_ulong,
                0,
                0,
            )
        };
        if rc2 != 0 {
            let err = unsafe { *libc::__errno_location() };
            return Err(ServeError::Config(
                format!("seccomp filter install failed errno={err}").into(),
            ));
        }
    }
    tracing::info!(n = SECCOMP_ALLOW.len(), "seccomp BPF allowlist installed");
    Ok(())
}

/// True if the Unix peer is the same EUID (or root talking to a dropped process).
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

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn seccomp_allowlist_contains_epoll_wait_and_not_execve() {
        assert!(
            SECCOMP_ALLOW.contains(&libc::SYS_epoll_wait)
                || SECCOMP_ALLOW.contains(&libc::SYS_epoll_pwait)
        );
        assert!(!SECCOMP_ALLOW.contains(&libc::SYS_execve));
        assert!(!seccomp_filter_bytes().is_empty());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn landlock_restrict_on_tempdir_does_not_panic() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("static");
        std::fs::create_dir(&root).unwrap();
        let rules = dir.path().join("rules.json");
        std::fs::write(&rules, b"[]").unwrap();
        let sock = dir.path().join("ctl.sock");
        let json = format!(
            r#"{{"bind":"127.0.0.1:0","static_root":"{}","rules_path":"{}","control_socket":"{}"}}"#,
            root.display(),
            rules.display(),
            sock.display()
        );
        let cfg = Config::from_json(json.as_bytes()).unwrap();
        assert!(!cfg.landlock);

        // Fork so restrict_self cannot sandbox the cargo-test process.
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0, "fork");
        if pid == 0 {
            let _ = unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) };
            match landlock_restrict(&cfg) {
                Ok(()) => unsafe { libc::_exit(0) },
                Err(_) => unsafe { libc::_exit(1) },
            }
        }
        let mut status: libc::c_int = 0;
        let w = unsafe { libc::waitpid(pid, &mut status, 0) };
        assert_eq!(w, pid);
        assert!(libc::WIFEXITED(status), "child signalled status={status}");
        assert_eq!(
            libc::WEXITSTATUS(status),
            0,
            "landlock_restrict failed in child"
        );
    }

    #[test]
    fn prepare_socket_dir_creates() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("run").join("atomos.sock");
        prepare_socket_dir(&sock).unwrap();
        assert!(sock.parent().unwrap().is_dir());
    }
}
