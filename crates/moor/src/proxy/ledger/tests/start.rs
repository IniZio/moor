//! What a Box's supervisor starts from, read through the ledger: the spec the controller
//! recorded, and one CA for the Box's whole life, kept sealed in the row.

use std::sync::Arc;

use moor_core::domain::decision::{Decision, NewBox};
use moor_core::seam::proxy_host::ProxySpec;
use moor_core::user::UserId;
use moor_core::{BoxId, BoxName};

use super::{box_ledger_with, world};
use crate::proxy::nono::ca;
use crate::vault::Vault;

fn new_box(w: &super::World) -> BoxId {
    let nt = NewBox {
        name: BoxName::parse_allow_bare("a").unwrap(),
        repo: None,
    };
    w.store.apply(Decision::insert(nt)).unwrap().box_id
}

#[test]
fn a_box_supervisor_reads_the_spec_the_controller_recorded() {
    let w = world();
    let id = new_box(&w);
    let on = box_ledger_with(&w.store, id, Arc::clone(&w.vault));
    assert_eq!(on.spec(), Ok(None));
    let spec = ProxySpec {
        box_id: id,
        box_handle: None,
        other_repos: Vec::new(),
        box_repo: None,
        rules: Vec::new(),
        user: UserId::new(1),
    };
    w.store.set_proxy_spec(id, &spec).unwrap();
    assert_eq!(on.spec(), Ok(Some(spec)));
}

#[test]
fn a_box_gets_one_ca_for_life_with_its_key_sealed_in_the_row() {
    let w = world();
    let id = new_box(&w);
    let on = box_ledger_with(&w.store, id, Arc::clone(&w.vault));
    let first = on.ca().unwrap();
    let state = tempfile::tempdir().unwrap();
    ca::install(state.path(), id, (&first.key_pem, &first.cert_pem)).expect("a usable CA");

    on.down();
    on.up();
    let again = on.ca().unwrap();
    assert_eq!(again.cert_pem, first.cert_pem);
    assert_eq!(again.key_pem, first.key_pem);

    let kept = w.store.proxy_ca(id).unwrap().unwrap();
    assert_eq!(kept.cert_pem, first.cert_pem);
    let key = &first.key_pem;
    let leaked = kept
        .sealed_key
        .windows(key.len())
        .any(|win| win == key.as_slice());
    assert!(!leaked, "the row holds the CA key in the clear");
    let other = Vault::load_or_create(&state.path().join("other.key")).unwrap();
    assert!(other.open_box_ca(id, &kept.sealed_key).is_err());
}
