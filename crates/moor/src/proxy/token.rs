//! The egress token a Box presents to the shared egress Proxy: `<id>.<mac>`, where `id` is 32
//! random bytes as base64url and `mac` is the hex HMAC-SHA256 of `id` under the egress key.
//!
//! The shared Proxy checks the MAC before it spends any rate budget or ledger call, so a made-up
//! token costs one HMAC. The store keeps only the SHA-256 of the whole token.

use std::fmt::Write as _;

use hmac::{Hmac, Mac as _};
use moor_core::{BoxId, Secret};
use sha2::Sha256;
use subtle::ConstantTimeEq as _;

use crate::store::{Store, StoreError};

/// Mints a token for `box_id` under `key` and records it, replacing the Box's last one; `None`
/// when the Box has no proxy row. The delivery into the Box marks it delivered.
pub(crate) fn mint(store: &Store, key: &[u8], box_id: BoxId) -> Result<Option<Secret>, String> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| format!("cannot mint the egress token: {e}"))?;
    let id = base64url(&bytes);
    let token = format!("{id}.{}", mac(key, id.as_bytes()));
    let recorded = store
        .set_egress_token(box_id, token.as_bytes())
        .map_err(|e: StoreError| e.to_string())?;
    Ok(recorded.then(|| Secret::new(token.into_bytes())))
}

/// What every front lease token starts with, so the shared Proxy tells one from a Box token.
const LEASE_PREFIX: &str = "lease.";

/// A new front lease token under `key`, `lease.<id>.<mac>`, signed as a Box token is so the
/// shared Proxy checks it before it spends any budget.
pub(crate) fn mint_lease(key: &[u8]) -> Result<String, String> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| format!("cannot mint a lease: {e}"))?;
    let id = format!("{LEASE_PREFIX}{}", base64url(&bytes));
    let sig = mac(key, id.as_bytes());
    Ok(format!("{id}.{sig}"))
}

/// Whether `token` has the shape of a front lease token.
pub(crate) fn is_lease(token: &[u8]) -> bool {
    token.starts_with(LEASE_PREFIX.as_bytes())
}

/// Whether `token` carries a MAC that `key` made; compared in constant time.
pub(crate) fn verify(key: &[u8], token: &[u8]) -> bool {
    let Some(dot) = token.iter().rposition(|b| *b == b'.') else {
        return false;
    };
    let (id, sent) = (
        token.get(..dot).unwrap_or_default(),
        token.get(dot + 1..).unwrap_or_default(),
    );
    !id.is_empty() && mac(key, id).as_bytes().ct_eq(sent).into()
}

/// A tag that names `key` without revealing it, never zero. The shared Proxy answers it on
/// `/ready`, and a delivery records it, so a controller notices a Proxy or a token under another
/// key.
pub(crate) fn key_id(key: &[u8]) -> i64 {
    let hex = mac(key, b"moor egress key id");
    i64::from_str_radix(hex.get(..15).unwrap_or_default(), 16).unwrap_or(0) + 1
}

/// The hex HMAC-SHA256 of `id` under `key`.
fn mac(key: &[u8], id: &[u8]) -> String {
    let Ok(mut m) = Hmac::<Sha256>::new_from_slice(key) else {
        return String::new();
    };
    m.update(id);
    m.finalize()
        .into_bytes()
        .iter()
        .fold(String::with_capacity(64), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
}

/// The base64url alphabet, without padding.
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Encode bytes as base64url without padding.
pub(crate) fn base64url(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let acc = chunk
            .iter()
            .chain(std::iter::repeat(&0))
            .take(3)
            .fold(0_u32, |a, b| (a << 8) | u32::from(*b));
        for shift in [18_u32, 12, 6, 0].iter().take(chunk.len() + 1) {
            let idx = usize::try_from((acc >> shift) & 63).unwrap_or(0);
            if let Some(c) = ALPHABET.get(idx) {
                out.push(char::from(*c));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use moor_core::domain::decision::{Decision, NewBox};
    use moor_core::BoxName;

    use super::{mac, mint, verify};
    use crate::store::{token_hash, Store};
    use crate::testing::clock::FakeClock;
    use crate::testing::rng::FakeRng;

    #[test]
    fn only_a_mac_under_the_same_key_verifies() {
        let token = format!("abc.{}", mac(b"k1", b"abc"));
        assert!(verify(b"k1", token.as_bytes()));
        assert!(!verify(b"k2", token.as_bytes()), "another key");
        assert!(
            !verify(b"k1", format!("xyz.{}", mac(b"k1", b"abc")).as_bytes()),
            "another id"
        );
        assert!(!verify(b"k1", b"abc"), "no MAC");
        assert!(
            !verify(b"k1", format!(".{}", mac(b"k1", b"")).as_bytes()),
            "no id"
        );
        assert!(!verify(b"k1", b"random-token"), "made up");
    }

    #[test]
    fn a_minted_token_verifies_and_replaces_the_last() {
        let dir = tempfile::tempdir().unwrap();
        let clock = std::sync::Arc::new(FakeClock::new(moor_core::seam::clock::Ts::from_millis(1)));
        let store = Store::open(&dir.path().join("moor.db"), clock).unwrap();
        let nt = NewBox {
            name: BoxName::parse_allow_bare("a").unwrap(),
            repo: None,
        };
        let a = store.apply(Decision::insert(nt)).unwrap().box_id;
        assert!(mint(&store, b"k", a).unwrap().is_none(), "no proxy row");
        drop(store.reserve(a, &FakeRng::default()));
        let first = mint(&store, b"k", a).unwrap().unwrap();
        assert!(verify(b"k", first.expose()));
        let second = mint(&store, b"k", a).unwrap().unwrap();
        assert_eq!(
            store.egress_token_box(&token_hash(first.expose())).unwrap(),
            None
        );
        assert_eq!(
            store
                .egress_token_box(&token_hash(second.expose()))
                .unwrap(),
            Some(a)
        );
    }
}
