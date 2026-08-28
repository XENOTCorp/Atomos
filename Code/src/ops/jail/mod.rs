//! Post-bind jail: drop root, no_new_privs, SO_PEERCRED on ctl. Linux.

use std::path::Path;

use crate::config::Config;
use crate::error::ServeError;

mod landlock;
mod privs;
mod seccomp;
use landlock::landlock_restrict;
use privs::{drop_privs, drop_remaining_caps};
use seccomp::seccomp_allowlist;

pub use privs::peer_euid_ok;
pub use seccomp::{seccomp_filter_bytes, SECCOMP_ALLOW};

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
