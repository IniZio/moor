//! Paths on the controller host that a request names. The client resolves them against its own
//! directory before it sends, so the controller takes only absolute paths.

use std::path::{Path, PathBuf};

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
