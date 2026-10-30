//! Paths on the controller host that a request names. The client resolves them against its own
//! directory before it sends, so the controller takes only absolute paths.

use std::path::{Component, Path, PathBuf};

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

#[cfg(test)]
mod tests;
