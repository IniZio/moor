//! moor's private directory of Unix sockets on the host.
//!
//! Any process that can open one of these sockets can use what it serves, so only moor's own
//! user may enter the directory.

use std::fmt::Write as _;
use std::fs::{self, DirBuilder};
use std::io::ErrorKind;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// Where moor's sockets live: `/tmp/moor-<uid>`. ssh first binds a control socket path plus
/// a 17-byte suffix, and macOS allows 103 bytes, so a socket under a deep state dir would fail
/// every call; this path stays short wherever the state dir is.
pub(crate) fn socket_dir() -> PathBuf {
    PathBuf::from(format!("/tmp/moor-{}", rustix::process::getuid().as_raw()))
}

/// The first 8 bytes of the SHA-256 of `parts`, each ended by a NUL byte, in hex.
pub(crate) fn short_hash(parts: &[&[u8]]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part);
        hasher.update([0]);
    }
    hasher
        .finalize()
        .iter()
        .take(8)
        .fold(String::new(), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// Creates `dir` for sockets, or checks the one there: it must be a directory of `owner` that
/// nobody else can enter.
pub(crate) fn private_dir(dir: &Path, owner: u32) -> Result<(), String> {
    match DirBuilder::new().mode(0o700).create(dir) {
        Err(e) if e.kind() != ErrorKind::AlreadyExists => {
            return Err(format!("cannot create {}: {e}", dir.display()));
        }
        _ => {}
    }
    let meta =
        fs::symlink_metadata(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
    let own = meta.is_dir() && meta.uid() == owner && meta.mode() & 0o777 == 0o700;
    if own {
        Ok(())
    } else {
        Err(format!(
            "{} is not a directory that only this user can enter",
            dir.display()
        ))
    }
}
