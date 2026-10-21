//! The vault key: one age X25519 identity in `vault.key` beside the controller config. It
//! seals each user secret before the store writes it and opens it on every read.

use std::fs::{self, OpenOptions};
use std::io::{self, Write as _};
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};
use std::str::FromStr as _;

use age::secrecy::ExposeSecret as _;
use age::x25519::{Identity, Recipient};
use moor_core::Secret;

/// File name of the vault key in the controller config's directory.
pub(crate) const VAULT_KEY: &str = "vault.key";

/// Why the vault key could not be used, or a value did not seal or open.
#[derive(Debug, thiserror::Error)]
pub(crate) enum VaultError {
    /// The key file is unusable; moor refuses it and leaves it as it is.
    #[error("vault key {}: {why}", path.display())]
    Key {
        /// The key file.
        path: PathBuf,
        /// What is wrong with it.
        why: String,
    },
    /// A stored value did not decrypt with this key.
    #[error("a vault value did not decrypt: {0}")]
    Open(String),
    /// A value did not encrypt.
    #[error("a vault value did not encrypt: {0}")]
    Seal(String),
}

/// The loaded vault key.
pub(crate) struct Vault {
    /// The private key.
    identity: Identity,
    /// Its public half, which seals.
    recipient: Recipient,
}

impl Vault {
    /// Loads the key at `path`, creating a new one mode 0600 when no file is there. A key that
    /// is a symlink, not a regular file, owned by another user, open to group or others, or
    /// not an age identity is refused and left as it is.
    pub(crate) fn load_or_create(path: &Path) -> Result<Self, VaultError> {
        load_or_create_as(path, rustix::process::geteuid().as_raw())
    }

    /// Encrypts `plain` to this key.
    pub(crate) fn seal(&self, plain: &Secret) -> Result<Vec<u8>, VaultError> {
        age::encrypt(&self.recipient, plain.expose()).map_err(|e| VaultError::Seal(e.to_string()))
    }

    /// Decrypts a value that [`Vault::seal`] made.
    pub(crate) fn open(&self, sealed: &[u8]) -> Result<Secret, VaultError> {
        age::decrypt(&self.identity, sealed)
            .map(Secret::new)
            .map_err(|e| VaultError::Open(e.to_string()))
    }
}

/// [`Vault::load_or_create`] with the uid that must own the file.
pub(super) fn load_or_create_as(path: &Path, owner: u32) -> Result<Vault, VaultError> {
    let refused = |why: String| VaultError::Key {
        path: path.to_path_buf(),
        why,
    };
    match create(path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(refused(format!("cannot create it: {e}"))),
    }
    let meta = fs::symlink_metadata(path).map_err(|e| refused(e.to_string()))?;
    if !meta.is_file() {
        return Err(refused("is not a regular file".into()));
    }
    if meta.uid() != owner {
        return Err(refused(format!(
            "is owned by uid {}, not {owner}",
            meta.uid()
        )));
    }
    if meta.mode() & 0o077 != 0 {
        return Err(refused(format!(
            "has mode {:o}; it must be 0600",
            meta.mode() & 0o777
        )));
    }
    let text = Secret::new(fs::read(path).map_err(|e| refused(e.to_string()))?);
    let identity = std::str::from_utf8(text.expose())
        .ok()
        .and_then(|t| Identity::from_str(t.trim()).ok())
        .ok_or_else(|| refused("is not an age X25519 identity".into()))?;
    Ok(Vault {
        recipient: identity.to_public(),
        identity,
    })
}

/// Writes a new key to `path` with mode 0600; fails with `AlreadyExists` when a file is there.
fn create(path: &Path) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    let identity = Identity::generate();
    writeln!(file, "{}", identity.to_string().expose_secret())?;
    file.sync_all()
}

#[cfg(test)]
mod tests;
