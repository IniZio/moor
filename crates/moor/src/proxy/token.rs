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

use crate::proxy::detached::base64url;
use crate::store::{Store, StoreError};

/// Mints a token for `box_id` under `key` and records it, replacing the Box's last one; `None`
/// when the Box has no proxy row. The delivery into the Box marks it delivered.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the in-Box forwarder's delivery mints its token")
)]
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
        drop(store.reserve(a, &FakeRng::default(), std::time::Duration::from_secs(30)));
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
