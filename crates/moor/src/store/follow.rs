//! Follow claims: an open run row is in flight exactly while someone holds its claim.
//!
//! The claim is an exclusive flock on `<state>/runs/<id>.follow`. The kernel drops it when the
//! holder dies, so a follower killed mid-run leaves a free claim behind an open row.

use std::fs::{self, File, TryLockError};
use std::io;
use std::path::{Path, PathBuf};

/// Exclusive hold on one run's follow file; released, and the file removed, on drop.
#[derive(Debug)]
pub(crate) struct FollowClaim {
    /// Open file carrying the flock.
    _file: File,
    /// The follow file, removed when the claim ends.
    path: PathBuf,
}

impl FollowClaim {
    /// Take the claim on `path`, creating its directory.
    ///
    /// # Errors
    /// The file cannot be opened, or another holder already has it.
    pub(super) fn take(path: PathBuf) -> io::Result<Self> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let file = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)?;
        file.try_lock()?;
        Ok(Self { _file: file, path })
    }
}

impl Drop for FollowClaim {
    fn drop(&mut self) {
        fs::remove_file(&self.path).ok();
    }
}

/// Whether someone holds the claim on `path`; a missing file is a free claim.
///
/// # Errors
/// The file exists but cannot be opened or probed.
pub(super) fn held(path: &Path) -> io::Result<bool> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    match file.try_lock_shared() {
        Ok(()) => Ok(false),
        Err(TryLockError::WouldBlock) => Ok(true),
        Err(TryLockError::Error(e)) => Err(e),
    }
}
