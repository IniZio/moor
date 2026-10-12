//! Secret bytes that never format, clone, serialise or compare.

use std::fmt::Write as _;

use sha2::{Digest as _, Sha256};
use zeroize::Zeroize;

/// Secret bytes; zeroized on drop, with no formatting, cloning or equality.
pub struct Secret(Box<[u8]>);

impl Secret {
    /// Wraps secret bytes.
    #[must_use]
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes.into_boxed_slice())
    }

    /// Returns the raw bytes; callers must never log or store them.
    #[must_use]
    pub fn expose(&self) -> &[u8] {
        &self.0
    }

    /// Returns a short non-reversible fingerprint, safe to compare and print.
    #[must_use]
    pub fn digest(&self) -> Digest {
        let hash = Sha256::digest(&self.0);
        let hex = hash
            .iter()
            .take(8)
            .fold(String::with_capacity(16), |mut acc, b| {
                let _ = write!(acc, "{b:02x}");
                acc
            });
        Digest(hex)
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// Lowercase hex of the first 8 bytes of the sha256 of a secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Digest(String);

impl Digest {
    /// Returns the 16-character hex string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests;
