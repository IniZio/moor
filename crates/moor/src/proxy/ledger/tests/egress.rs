//! The shared egress Proxy's end of the ledger: its key opens token lookups and the reads a
//! request makes for one Box, and nothing else.

use std::collections::BTreeMap;

use moor_core::config::proxy::{ResolvedInject, ResolvedRule};
use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, Field, Fields, NewBox};
use moor_core::seam::proxy_host::ProxySpec;
use moor_core::user::SecretName;
use moor_core::{BoxHandle, BoxId, BoxName, ProviderId};

use super::super::{BoxLedger, EgressLedger, Endpoint, LedgerError};
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
    drop(w.store.reserve(id, &FakeRng::default()));
    assert!(w.store.set_egress_token(id, token).unwrap());
    id
}

#[test]
fn a_token_finds_its_box_until_the_box_closes() {
    let w = world();
    let _served = w.serve();
    let a = box_with_token(&w, "a", b"token-a");
    let ledger = EgressLedger::at(Endpoint::Unix(w.socket.clone()), EGRESS_KEY.to_owned());
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
fn the_awake_tokens_name_every_open_awake_box_and_skip_a_row_that_cannot_be_read() {
    let w = world();
    let _served = w.serve();
    let closed = box_with_token(&w, "closed", b"token-closed");
    let sleeping = box_with_token(&w, "sleeping", b"token-sleeping");
    let broken = box_with_token(&w, "broken", b"token-broken");
    let awake = box_with_token(&w, "awake", b"token-awake");
    let ledger = EgressLedger::at(Endpoint::Unix(w.socket.clone()), EGRESS_KEY.to_owned());
    assert_eq!(ledger.awake_tokens().unwrap().tokens.len(), 4);
    let close = Decision::new(closed).lifecycle(BoxLifecycleInput::Close { reason: None });
    w.store.apply(close).unwrap();
    asleep(&w, sleeping);
    w.store.damage_box_state(broken, "unreadable");
    let left = ledger.awake_tokens().unwrap();
    assert_eq!(left.tokens, vec![(token_hash(b"token-awake"), awake)]);
    let unread: Vec<BoxId> = left.unread.iter().map(|(id, _)| *id).collect();
    assert_eq!(unread, [broken]);
    let wrong = EgressLedger::at(Endpoint::Unix(w.socket), "not-the-key".to_owned());
    assert!(matches!(wrong.awake_tokens(), Err(LedgerError::Refused(_))));
}

/// Takes `id` up on a machine, then to sleep.
fn asleep(w: &World, id: BoxId) {
    let up = Decision::new(id)
        .box_input(BoxInput::Requested)
        .set(Fields {
            box_provider: Field::Set(ProviderId::new("p")),
            ..Fields::default()
        });
    w.store.apply(up).unwrap();
    let acquired = Decision::new(id).box_input(BoxInput::Acquired {
        handle: BoxHandle::new("p/b"),
    });
    w.store.apply(acquired).unwrap();
    w.store
        .apply(Decision::new(id).box_input(BoxInput::Sleep))
        .unwrap();
}

#[test]
fn a_wrong_egress_key_is_refused() {
    let w = world();
    let _served = w.serve();
    box_with_token(&w, "a", b"token-a");
    let ledger = EgressLedger::at(Endpoint::Unix(w.socket), "not-the-key".to_owned());
    assert!(matches!(
        ledger.token_box(&token_hash(b"token-a")),
        Err(LedgerError::Refused(_))
    ));
}

#[test]
fn the_egress_proxy_reads_a_box() {
    let w = world();
    let _served = w.serve();
    let a = box_with_token(&w, "a", b"token-a");
    let ledger = BoxLedger::egress(Endpoint::Unix(w.socket), a, EGRESS_KEY.to_owned());
    assert!(ledger.view().unwrap().is_some_and(|v| v.open));
}

#[test]
fn the_egress_proxy_reads_only_secrets_its_rules_name_and_only_while_the_box_is_open() {
    let w = world();
    let _served = w.serve();
    let nt = NewBox {
        name: BoxName::parse_allow_bare("a").unwrap(),
        repo: None,
    };
    let a = w
        .store
        .apply(Decision::insert(nt).requester(w.ann))
        .unwrap()
        .box_id;
    drop(w.store.reserve(a, &FakeRng::default()));
    let rule = ResolvedRule {
        name: "claude".to_owned(),
        hosts: vec!["api.anthropic.com".to_owned()],
        credential: "claude".to_owned(),
        inject: ResolvedInject::Header {
            header: "x-api-key".to_owned(),
            form: "{secret}".to_owned(),
        },
        endpoint_rules: Vec::new(),
        box_env: BTreeMap::new(),
    };
    let spec = ProxySpec {
        box_id: a,
        box_handle: None,
        box_repo: None,
        other_repos: Vec::new(),
        rules: vec![rule],
        user: w.ann,
    };
    w.store.set_proxy_spec(a, &spec).unwrap();
    let ledger = BoxLedger::egress(Endpoint::Unix(w.socket.clone()), a, EGRESS_KEY.to_owned());
    let named = ledger
        .secret(&SecretName::parse("claude").unwrap())
        .unwrap();
    assert_eq!(named.unwrap().expose(), b"ann-token");
    let other = ledger.secret(&SecretName::parse("github").unwrap());
    assert!(
        matches!(other, Err(LedgerError::Refused(_))),
        "a secret no rule names"
    );
    let close = Decision::new(a).lifecycle(BoxLifecycleInput::Close { reason: None });
    w.store.apply(close).unwrap();
    assert!(
        matches!(ledger.spec(), Err(LedgerError::Refused(_))),
        "a closed Box"
    );
    assert!(
        ledger.view().unwrap().is_some_and(|v| !v.open),
        "its view still answers"
    );
}
