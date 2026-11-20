//! The App's webhook secret and the signature check GitHub's deliveries pass.
//!
//! GitHub signs each delivery's body with HMAC-SHA256 under the secret and sends
//! `sha256=<hex>` in `X-Hub-Signature-256`. The check compares in constant time.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use hmac::{Hmac, Mac as _};
use sha2::Sha256;
use subtle::ConstantTimeEq as _;
use zeroize::Zeroizing;

/// The App's webhook secret, never empty.
pub(crate) struct HookSecret(Zeroizing<Vec<u8>>);

impl HookSecret {
    /// The secret in `path`, without the whitespace around it. `expand` resolves `~`.
    ///
    /// # Errors
    /// The file cannot be read or holds only whitespace. The error names the file, never its
    /// content.
    pub(crate) fn load(path: &Path, expand: impl Fn(&Path) -> PathBuf) -> Result<Self, String> {
        let path = expand(path);
        let read = Zeroizing::new(
            fs::read(&path)
                .map_err(|e| format!("github_app: cannot read {}: {e}", path.display()))?,
        );
        Self::new(read.trim_ascii())
            .ok_or_else(|| format!("github_app: {} is empty", path.display()))
    }

    /// The secret `bytes`; none when they are empty, so no check runs with an empty key.
    pub(crate) fn new(bytes: &[u8]) -> Option<Self> {
        (!bytes.is_empty()).then(|| Self(Zeroizing::new(bytes.to_vec())))
    }

    /// Whether `header` is `sha256=` and the lowercase hex HMAC-SHA256 of `body` under this
    /// secret. A missing header never matches.
    pub(crate) fn signed(&self, body: &[u8], header: Option<&str>) -> bool {
        let Some(header) = header else {
            return false;
        };
        let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(&self.0) else {
            return false;
        };
        mac.update(body);
        let want = mac
            .finalize()
            .into_bytes()
            .iter()
            .fold(String::from("sha256="), |mut s, b| {
                let _: std::fmt::Result = write!(s, "{b:02x}");
                s
            });
        bool::from(want.as_bytes().ct_eq(header.trim().as_bytes()))
    }
}

#[cfg(test)]
mod tests;
