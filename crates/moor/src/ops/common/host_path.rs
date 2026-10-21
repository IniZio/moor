//! Paths on the controller host that a request names. The client resolves them against its own
//! directory before it sends, so the controller takes only absolute paths.

use std::path::{Path, PathBuf};

use moor_core::ErrorKind;

use crate::cx::Cx;
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

/// The directory the caller's `in` request names.
///
/// # Errors
/// Usage for a request that names no repo and no directory.
pub(crate) fn caller_dir(cx: &Cx) -> Result<&Path, CommandError> {
    cx.dir().ok_or_else(|| {
        CommandError::new(
            ErrorKind::Usage,
            "no repo: name one with --repo, or run the CLI inside a checkout",
        )
    })
}
