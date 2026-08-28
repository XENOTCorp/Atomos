//! Landlock filesystem restrict after bind.
use std::ffi::CString;
use std::path::Path;
use crate::config::Config;
use crate::error::ServeError;

pub(crate) const SYS_LANDLOCK_CREATE_RULESET: libc::c_long = 444;
#[cfg(target_os = "linux")]
pub(crate) const SYS_LANDLOCK_ADD_RULE: libc::c_long = 445;
#[cfg(target_os = "linux")]
pub(crate) const SYS_LANDLOCK_RESTRICT_SELF: libc::c_long = 446;

#[cfg(target_os = "linux")]
pub(crate) const LANDLOCK_ACCESS_FS_WRITE_FILE: u64 = 1 << 1;
#[cfg(target_os = "linux")]
pub(crate) const LANDLOCK_ACCESS_FS_READ_FILE: u64 = 1 << 2;
#[cfg(target_os = "linux")]
pub(crate) const LANDLOCK_ACCESS_FS_READ_DIR: u64 = 1 << 3;
#[cfg(target_os = "linux")]
pub(crate) const LANDLOCK_RULE_PATH_BENEATH: i32 = 1;

#[cfg(target_os = "linux")]
#[repr(C)]
pub(crate) struct LandlockRulesetAttr {
    handled_access_fs: u64,
}

#[cfg(target_os = "linux")]
#[repr(C, packed)]
pub(crate) struct LandlockPathBeneathAttr {
    allowed_access: u64,
    parent_fd: i32,
}

#[cfg(target_os = "linux")]
pub(crate) fn landlock_unsupported(err: i32) -> bool {
    err == libc::ENOSYS || err == libc::EOPNOTSUPP
}

#[cfg(target_os = "linux")]
pub(crate) fn landlock_errno() -> i32 {
    unsafe { *libc::__errno_location() }
}

#[cfg(target_os = "linux")]
pub(crate) fn path_parent_for_rule(path: &Path) -> Option<&Path> {
    match path.parent() {
        None => None,
        Some(p) if p.as_os_str().is_empty() => Some(Path::new(".")),
        Some(p) => Some(p),
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn landlock_add_path(ruleset_fd: libc::c_int, path: &Path, allowed: u64) -> Result<(), ServeError> {
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
pub(crate) fn landlock_restrict(cfg: &Config) -> Result<(), ServeError> {
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
