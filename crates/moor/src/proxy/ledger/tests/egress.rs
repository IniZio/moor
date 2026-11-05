//! The shared egress Proxy's end of the ledger: its key opens token lookups and the reads a
//! request makes for one Box, and nothing else.

use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::decision::{Decision, NewBox};
use moor_core::{BoxId, BoxName};

use super::super::{BoxLedger, EgressLedger, LedgerError};
use super::{world, World, EGRESS_KEY};
use crate::store::token_hash;
use crate::testing::rng::FakeRng;

/// A Box with a proxy row whose egress token is `token`.
fn box_with_token(w: &World, name: &str, token: &[u8]) -> BoxId {
    let nt = NewBox {
        name: BoxName::parse_allow_bare(name).unwrap(),
        repo: None,
    };
    let id = w.store.apply(Decision::insert(nt)).unwrap().box_id;
    drop(
        w.store
            .reserve(id, &FakeRng::default(), crate::proxy::detached::LEASE),
    );
    assert!(w.store.set_egress_token(id, token).unwrap());
    id
}

#[test]
fn a_token_finds_its_box_until_the_box_closes() {
    let w = world();
    let _served = w.serve();
    let a = box_with_token(&w, "a", b"token-a");
    let ledger = EgressLedger::at(w.socket.clone(), EGRESS_KEY.to_owned());
    assert_eq!(ledger.token_box(&token_hash(b"token-a")), Ok(Some(a)));
    assert_eq!(ledger.token_box(&token_hash(b"token-b")), Ok(None));
    let close = Decision::new(a).lifecycle(BoxLifecycleInput::Close { reason: None });
    w.store.apply(close).unwrap();
    assert_eq!(
        ledger.token_box(&token_hash(b"token-a")),
        Ok(None),
        "a closed Box's token"
    );
}

#[test]
fn a_wrong_egress_key_is_refused() {
    let w = world();
    let _served = w.serve();
    box_with_token(&w, "a", b"token-a");
    let ledger = EgressLedger::at(w.socket.clone(), "not-the-key".to_owned());
    assert!(matches!(
        ledger.token_box(&token_hash(b"token-a")),
        Err(LedgerError::Refused(_))
    ));
}

#[test]
fn the_egress_proxy_reads_a_box_but_never_takes_its_lease() {
    let w = world();
    let _served = w.serve();
    let a = box_with_token(&w, "a", b"token-a");
    let ledger = BoxLedger::egress(w.socket.clone(), a, EGRESS_KEY.to_owned());
    assert!(ledger.view().unwrap().is_some_and(|v| v.open));
    assert!(matches!(
        ledger.claim_lease("owner", std::time::Duration::from_secs(30)),
        Err(LedgerError::Refused(_))
    ));
    assert!(matches!(ledger.box_port(), Err(LedgerError::Refused(_))));
}
