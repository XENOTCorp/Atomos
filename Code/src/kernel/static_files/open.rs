//! Race-resistant root-relative opens. Never canonicalize then reopen a path.
use rustix::fs::{open, openat, openat2, Mode, OFlags, ResolveFlags};
use std::fs::File;
use std::io;
use std::os::fd::AsFd;
use std::path::{Component, Path};

pub(super) fn root(path: &Path) -> io::Result<File> {
    open(
        path,
        OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(Into::into)
}

pub(super) fn beneath(root: &File, relative: &Path) -> io::Result<File> {
    let flags = OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK;
    match openat2(
        root,
        relative,
        flags,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_MAGICLINKS,
    ) {
        Ok(fd) => Ok(File::from(fd)),
        Err(rustix::io::Errno::NOSYS) => without_symlinks(root, relative),
        Err(error) => Err(error.into()),
    }
}

/// Old kernels: walk owned directory descriptors with NOFOLLOW on every
/// component. This fallback intentionally disallows even internal symlinks.
fn without_symlinks(root: &File, relative: &Path) -> io::Result<File> {
    let mut directory = None;
    let mut parts = relative
        .components()
        .filter(|part| *part != Component::CurDir)
        .peekable();
    while let Some(part) = parts.next() {
        let Component::Normal(name) = part else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid static path",
            ));
        };
        let last = parts.peek().is_none();
        let mode = if last {
            OFlags::RDONLY | OFlags::NONBLOCK
        } else {
            OFlags::PATH | OFlags::DIRECTORY
        };
        let parent = directory.as_ref().map_or_else(|| root.as_fd(), AsFd::as_fd);
        let fd = openat(
            parent,
            name,
            mode | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )?;
        if last {
            return Ok(File::from(fd));
        }
        directory = Some(fd);
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        "empty static path",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_open_strategies_reject_parent_and_symlink_escape() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret"), b"secret").unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("escape")).unwrap();
        std::fs::create_dir(dir.path().join("nested")).unwrap();
        std::fs::write(dir.path().join("nested/ok"), b"ok").unwrap();
        let root = root(dir.path()).unwrap();
        for open in [beneath, without_symlinks] {
            assert!(open(&root, Path::new("escape/secret")).is_err());
            assert!(open(&root, Path::new("../secret")).is_err());
            assert!(open(&root, Path::new("nested/ok")).is_ok());
        }
    }
}
