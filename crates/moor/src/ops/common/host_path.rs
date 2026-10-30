//! Paths on the controller host that a request names. The client resolves them against its own
//! directory before it sends, so the controller takes only absolute paths.

use std::fs::File;
use std::path::{Component, Path, PathBuf};

use rustix::fs::{openat, Mode, OFlags, CWD};
use rustix::io::Errno;

use moor_core::ErrorKind;

use crate::error::CommandError;

/// `path`, which `what` names, when it is absolute.
///
/// # Errors
/// Usage for a relative path.
pub(crate) fn absolute(what: &str, path: &str) -> Result<PathBuf, CommandError> {
    let p = Path::new(path);
    if p.is_absolute() {
        return Ok(p.to_path_buf());
    }
    Err(CommandError::new(
        ErrorKind::Usage,
        format!("{what} {path} is not an absolute path"),
    ))
}

/// `path`, which `what` names, with the symlinks in the part of it that exists resolved, when
/// that lies inside `root`.
///
/// # Errors
/// Usage for a relative path, refused for a `..`, an unresolvable or missing `root`, a dangling
/// symlink, or a path that resolves outside `root`.
pub(crate) fn inside(what: &str, path: &str, root: &Path) -> Result<PathBuf, CommandError> {
    let p = absolute(what, path)?;
    let refused = |why: String| CommandError::new(ErrorKind::Refused, why);
    if p.components().any(|c| c == Component::ParentDir) {
        return Err(refused(format!("{what} {path} must not contain `..`")));
    }
    let root = root
        .canonicalize()
        .map_err(|e| refused(format!("cannot resolve {}: {e}", root.display())))?;
    let mut existing = p;
    let mut missing = Vec::new();
    while existing.symlink_metadata().is_err() {
        let Some(name) = existing.file_name().map(ToOwned::to_owned) else {
            break;
        };
        missing.push(name);
        existing.pop();
    }
    let mut real = existing
        .canonicalize()
        .map_err(|e| refused(format!("cannot resolve {what} {path}: {e}")))?;
    real.extend(missing.iter().rev());
    if !real.starts_with(&root) {
        return Err(refused(format!(
            "{what} {path} is outside {}",
            root.display()
        )));
    }
    Ok(real)
}

/// `path`, which `what` names, opened to read when it lies inside `root`. The open walks down
/// from `root` one name at a time without following a link, so a link swapped in after the
/// check cannot lead it out.
///
/// # Errors
/// What [`inside`] refuses, refused when the walk meets a link, or usage when a name will not
/// open.
pub(crate) fn open_beneath(what: &str, path: &str, root: &Path) -> Result<File, CommandError> {
    let real = inside(what, path, root)?;
    let refused = |why: String| CommandError::new(ErrorKind::Refused, why);
    let root = root
        .canonicalize()
        .map_err(|e| refused(format!("cannot resolve {}: {e}", root.display())))?;
    let under = real
        .strip_prefix(&root)
        .map_err(|_| refused(format!("{what} {path} is outside {}", root.display())))?;
    walk(&root, under).map_err(|e| {
        if Errno::from_io_error(&e) == Some(Errno::LOOP) {
            refused(format!("{what} {path} changed into a link"))
        } else {
            CommandError::new(ErrorKind::Usage, format!("cannot read {what} {path}: {e}"))
        }
    })
}

/// `rel` under the directory `root`, opened one name at a time with no link followed and
/// without blocking on a FIFO.
fn walk(root: &Path, rel: &Path) -> std::io::Result<File> {
    let flags = OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW;
    let mut dir = openat(CWD, root, flags | OFlags::DIRECTORY, Mode::empty())?;
    let mut names = rel.components().peekable();
    while let Some(name) = names.next() {
        let Component::Normal(name) = name else {
            return Err(std::io::ErrorKind::InvalidInput.into());
        };
        let last = names.peek().is_none();
        let kind = if last {
            OFlags::NONBLOCK
        } else {
            OFlags::DIRECTORY
        };
        dir = openat(&dir, name, flags | kind, Mode::empty())?;
    }
    Ok(File::from(dir))
}

#[cfg(test)]
mod tests;
