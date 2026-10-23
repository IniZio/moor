//! A host Session's own directory under the controller's state, made one level at a time
//! so that no symlink under the state is followed and nothing someone else made is reused.

use std::fs::{self, DirBuilder};
use std::io;
use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, PermissionsExt as _};
use std::path::Path;

/// Creates `dir`, which lies under `state`, and each missing level between them, mode 0700.
/// A level that exists must be a real directory owned by this process's user; the last one
/// must also be mode 0700.
///
/// # Errors
/// A level that is a symlink or not a directory, owned by another user, a last level that
/// already exists with another mode, `dir` outside `state`, or the IO failure.
pub(super) fn create(state: &Path, dir: &Path) -> io::Result<()> {
    let rest = dir
        .strip_prefix(state)
        .map_err(|_| io::Error::other(format!("{} is not under the state", dir.display())))?;
    let me = rustix::process::geteuid().as_raw();
    let levels = rest.components().count();
    let mut at = state.to_path_buf();
    for (n, part) in rest.components().enumerate() {
        at.push(part);
        match DirBuilder::new().mode(0o700).create(&at) {
            Ok(()) => continue,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
        let meta = fs::symlink_metadata(&at)?;
        let why = if meta.file_type().is_symlink() || !meta.is_dir() {
            "is not a real directory"
        } else if meta.uid() != me {
            "belongs to another user"
        } else if n + 1 == levels && meta.permissions().mode() & 0o777 != 0o700 {
            "already exists and is not mode 0700"
        } else {
            continue;
        };
        return Err(io::Error::other(format!("{} {why}", at.display())));
    }
    Ok(())
}
