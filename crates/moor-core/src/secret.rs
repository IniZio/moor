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

/// What every token `claude setup-token` prints starts with.
const CLAUDE_PREFIX: &[u8] = b"sk-ant-";

/// Longest Claude token taken.
const CLAUDE_MAX: usize = 512;

/// Why `token` cannot be what `claude setup-token` prints, found without asking Anthropic: it
/// starts with `sk-ant-` and holds only letters, digits, `-` and `_`.
///
/// # Errors
/// The reason, as a clause that follows "it".
pub fn check_claude_token(token: &Secret) -> Result<(), String> {
    let bytes = token.expose();
    let shaped = bytes.starts_with(CLAUDE_PREFIX)
        && bytes.len() <= CLAUDE_MAX
        && bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'));
    if shaped {
        Ok(())
    } else {
        Err("is not a token that `claude setup-token` prints".to_owned())
    }
}

/// Why `token` cannot be what `gh auth token` prints, found without asking GitHub: it must be
/// one visible ASCII word.
///
/// # Errors
/// The reason, as a clause that follows "it".
pub fn check_github_token(token: &Secret) -> Result<(), String> {
    let bytes = token.expose();
    if bytes.is_empty() {
        return Err("is empty".to_owned());
    }
    if !bytes.iter().all(u8::is_ascii_graphic) {
        return Err("is not the one word `gh auth token` prints".to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
