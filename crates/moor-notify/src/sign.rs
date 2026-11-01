//! The signature a receiver checks: HMAC-SHA256 over `<timestamp>.<body>`.

use hmac::{Hmac, Mac as _};
use sha2::Sha256;

/// The `X-Moor-Signature` value for `body` sent at unix second `timestamp`.
///
/// It is `sha256=` and the lowercase hex HMAC-SHA256 of `<timestamp>.<body>` under `key`.
/// The timestamp is in the signed text so a receiver can refuse an old delivery replayed
/// later.
#[must_use]
pub fn signature(key: &[u8], timestamp: u64, body: &[u8]) -> String {
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(key) else {
        unreachable!("HMAC takes a key of any length")
    };
    mac.update(timestamp.to_string().as_bytes());
    mac.update(b".");
    mac.update(body);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

#[cfg(test)]
mod tests {
    use super::signature;

    /// The value `openssl` gives for the same key and text, so a receiver in any language
    /// that follows the docs computes the same string.
    #[test]
    fn matches_openssl() {
        // printf '1700000000.{"a":1}' | openssl dgst -sha256 -hmac k
        assert_eq!(
            signature(b"k", 1_700_000_000, br#"{"a":1}"#),
            "sha256=1b6ad1bc9bf48c52ce01939866d5877c6cc4e87ddad27e16673cb8c4964394a3"
        );
    }
}
