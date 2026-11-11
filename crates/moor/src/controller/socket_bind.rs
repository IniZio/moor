//! Binding the controller's Unix sockets.

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};

use super::mkdir;

/// Replaces any stale socket file with a fresh owner-only listener.
///
/// The listener is bound beside `socket` and renamed over it, so the path always names the old
/// socket (a refused dial) or the new one, never nothing.
pub(super) fn bind(socket: &Path) -> std::io::Result<UnixListener> {
    if let Some(dir) = socket.parent() {
        mkdir(dir)?;
    }
    let mut staged = socket.as_os_str().to_owned();
    staged.push("~");
    let staged = PathBuf::from(staged);
    // The lock proves no live controller owns this path.
    match fs::remove_file(&staged) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    let listener = UnixListener::bind(&staged)?;
    fs::set_permissions(&staged, fs::Permissions::from_mode(0o600))?;
    fs::rename(&staged, socket)?;
    Ok(listener)
}
