//! Filesystem permissions for the store: the db holds the proxy's secret
//! nonce, so the state dir is 0700 and the db and its WAL files are 0600,
//! also when they already exist with looser modes.

use std::fs::{self, DirBuilder, OpenOptions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// Create the state dir (0700) and the db file (0600) if missing, and tighten both if they exist with looser modes.
pub(super) fn prepare(db: &Path) -> io::Result<()> {
    if let Some(dir) = db.parent().filter(|d| !d.as_os_str().is_empty()) {
        DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(db)?;
    fs::set_permissions(db, fs::Permissions::from_mode(0o600))
}

/// Set the db file and its `-wal` and `-shm` siblings, where present, to 0600.
pub(super) fn tighten(db: &Path) -> io::Result<()> {
    for path in [db.to_path_buf(), sibling(db, "-wal"), sibling(db, "-shm")] {
        match fs::set_permissions(&path, fs::Permissions::from_mode(0o600)) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            other => other?,
        }
    }
    Ok(())
}

/// The db path with `suffix` appended to its full file name.
fn sibling(db: &Path, suffix: &str) -> PathBuf {
    let mut name = db.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

#[cfg(test)]
mod tests;
