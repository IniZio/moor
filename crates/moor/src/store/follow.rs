//! Follow claims: an open run row is in flight exactly while someone holds its claim.
//!
//! The claim is an exclusive flock on `<state>/runs/<id>.follow`. The kernel drops it when the
//! holder dies, so a follower killed mid-run leaves a free claim behind an open row.
//!
//! The holder removes the file when it lets go, so a lock counts only while it is on the file
//! currently at the path: a taker that opened the file before the removal locks a dead inode.

use std::fs::{self, File, TryLockError};
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// How many times a take or probe reopens the file after finding it replaced.
const TRIES: usize = 8;

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
        Self::take_between(path, &mut || {})
    }

    /// [`Self::take`], calling `opened` after each open of the file and before its lock.
    fn take_between(path: PathBuf, opened: &mut dyn FnMut()) -> io::Result<Self> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        for _ in 0..TRIES {
            let file = File::options()
                .create(true)
                .truncate(false)
                .write(true)
                .open(&path)?;
            opened();
            file.try_lock()?;
            if at_path(&file, &path)? {
                return Ok(Self { _file: file, path });
            }
        }
        Err(replaced(&path))
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
    held_between(path, &mut || {})
}

/// [`held`], calling `opened` after each open of the file and before its probe.
fn held_between(path: &Path, opened: &mut dyn FnMut()) -> io::Result<bool> {
    for _ in 0..TRIES {
        let file = match File::open(path) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e),
        };
        opened();
        let locked = match file.try_lock_shared() {
            Ok(()) => false,
            Err(TryLockError::WouldBlock) => true,
            Err(TryLockError::Error(e)) => return Err(e),
        };
        if at_path(&file, path)? {
            return Ok(locked);
        }
    }
    Err(replaced(path))
}

/// Whether `file` is the file at `path` now; a missing path is not.
fn at_path(file: &File, path: &Path) -> io::Result<bool> {
    let open = file.metadata()?;
    match fs::metadata(path) {
        Ok(now) => Ok((open.dev(), open.ino()) == (now.dev(), now.ino())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// The error when `path` was replaced on every one of [`TRIES`] opens.
fn replaced(path: &Path) -> io::Error {
    io::Error::other(format!(
        "{} kept being replaced while it was opened",
        path.display()
    ))
}

#[cfg(test)]
mod tests;
