//! Filesystem helpers for retiring an old-format state database: a lock that
//! keeps two processes from retiring at once, and a move that never overwrites.

use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

/// Takes a blocking exclusive lock on `<db>.retire.lock`, creating the
/// database's parent directory first if it is missing.
///
/// Hold the returned file from the version check through the fresh store's
/// schema init so two processes cannot both retire. The lock is released when
/// the file is dropped.
pub(super) fn lock_retire(db: &Path) -> io::Result<File> {
    if let Some(parent) = db.parent().filter(|p| !p.as_os_str().is_empty()) {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)?;
    }
    let mut name = db.as_os_str().to_owned();
    name.push(".retire.lock");
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(PathBuf::from(name))?;
    file.lock()?;
    Ok(file)
}

/// Moves `from` to `to`, failing with `AlreadyExists` instead of overwriting
/// when `to` is already there.
///
/// The target is claimed with a hard link, which fails if the name is taken;
/// only then is the source removed.
pub(super) fn move_noreplace(from: &Path, to: &Path) -> io::Result<()> {
    fs::hard_link(from, to)?;
    fs::remove_file(from)
}

#[cfg(test)]
mod tests;
