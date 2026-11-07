//! Cookie values the gate seals, so a browser holds its own sign-in state and any gate
//! replica given the same key can read it. ChaCha20-Poly1305 hides and authenticates the
//! value, and the purpose is authenticated with it so one kind of cookie never opens as another.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chacha20poly1305::aead::{Aead, AeadCore, KeyInit, OsRng, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The shortest secret a key may be made from.
const KEY_MIN: usize = 32;

/// Bytes of nonce ahead of the ciphertext.
const NONCE_LEN: usize = 12;

/// A value and the second it stops being valid, as sealed.
#[derive(Serialize, Deserialize)]
struct Envelope<T> {
    /// Unix seconds.
    exp: u64,
    /// What was sealed.
    value: T,
}

/// The keys cookies are sealed with: the first seals, every one opens.
pub(super) struct Seal {
    /// Newest first.
    keys: Vec<ChaCha20Poly1305>,
}

impl Seal {
    /// Keys from comma-separated secrets, each at least [`KEY_MIN`] characters.
    pub(super) fn from_keys(secrets: &str) -> Result<Self, String> {
        let keys: Vec<_> = secrets
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|secret| {
                if secret.len() < KEY_MIN {
                    return Err(format!("a cookie key must be {KEY_MIN} or more characters"));
                }
                Ok(ChaCha20Poly1305::new(&Sha256::digest(secret)))
            })
            .collect::<Result<_, _>>()?;
        if keys.is_empty() {
            return Err("the cookie key is empty".to_owned());
        }
        Ok(Self { keys })
    }

    /// A key no other gate has, for a gate that runs alone.
    pub(super) fn random() -> Self {
        Self {
            keys: vec![ChaCha20Poly1305::new(&ChaCha20Poly1305::generate_key(
                &mut OsRng,
            ))],
        }
    }

    /// `value` sealed under `purpose`, valid for `ttl` from `now`, as a cookie-safe string.
    pub(super) fn seal<T: Serialize>(
        &self,
        purpose: &str,
        value: &T,
        now: SystemTime,
        ttl: Duration,
    ) -> Result<String, String> {
        let exp = unix(now).saturating_add(ttl.as_secs());
        let plain = serde_json::to_vec(&Envelope { exp, value }).map_err(|e| e.to_string())?;
        let key = self.keys.first().ok_or("no cookie key")?;
        let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);
        let sealed = key
            .encrypt(
                &nonce,
                Payload {
                    msg: &plain,
                    aad: purpose.as_bytes(),
                },
            )
            .map_err(|e| e.to_string())?;
        let mut out = nonce.to_vec();
        out.extend(sealed);
        Ok(URL_SAFE_NO_PAD.encode(out))
    }

    /// The value in `cookie`, when a key opens it for `purpose` and it has not expired.
    pub(super) fn open<T: DeserializeOwned>(
        &self,
        purpose: &str,
        cookie: &str,
        now: SystemTime,
    ) -> Option<T> {
        let raw = URL_SAFE_NO_PAD.decode(cookie).ok()?;
        let (nonce, sealed) = raw.split_at_checked(NONCE_LEN)?;
        let nonce = Nonce::clone_from_slice(nonce);
        let plain = self.keys.iter().find_map(|key| {
            key.decrypt(
                &nonce,
                Payload {
                    msg: sealed,
                    aad: purpose.as_bytes(),
                },
            )
            .ok()
        })?;
        let envelope: Envelope<T> = serde_json::from_slice(&plain).ok()?;
        (unix(now) < envelope.exp).then_some(envelope.value)
    }
}

/// `time` in whole seconds since the Unix epoch.
fn unix(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}
