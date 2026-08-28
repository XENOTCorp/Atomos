//! seccomp allowlist after bind.
use crate::error::ServeError;

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
pub(crate) fn bpf_stmt(code: u32, k: u32) -> libc::sock_filter {
    libc::sock_filter {
        code: code as u16,
        jt: 0,
        jf: 0,
        k,
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn bpf_jump(code: u32, k: u32, jt: u8, jf: u8) -> libc::sock_filter {
    libc::sock_filter {
        code: code as u16,
        jt,
        jf,
        k,
    }
}

/// AUDIT_ARCH_X86_64: not always exported by libc.
#[cfg(target_os = "linux")]
pub(crate) const AUDIT_ARCH_X86_64: u32 = 0xc000_003e;

#[cfg(target_os = "linux")]
pub(crate) fn seccomp_filter_prog() -> Vec<libc::sock_filter> {
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
pub(crate) fn seccomp_allowlist() -> Result<(), ServeError> {
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

