//! A Proxy started before its row kept the spec and CA keeps both across the upgrade.

use std::sync::Arc;

use moor_core::domain::decision::{Decision, NewBox};
use moor_core::seam::clock::Ts;
use moor_core::seam::proxy_host::ProxySpec;
use moor_core::user::UserId;
use moor_core::BoxName;

use super::import;
use crate::proxy::nono::ca;
use crate::store::{Store, STORE_FILE};
use crate::testing::clock::FakeClock;
use crate::testing::rng::FakeRng;
use crate::vault::Vault;

#[test]
fn a_spec_and_ca_left_in_files_move_into_the_row_and_open_again() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1000)));
    let store = Store::open(&dir.path().join(STORE_FILE), clock).unwrap();
    let vault = Vault::load_or_create(&dir.path().join("vault.key")).unwrap();
    let nt = NewBox {
        name: BoxName::parse_allow_bare("old").unwrap(),
        repo: None,
    };
    let id = store.apply(Decision::insert(nt)).unwrap().box_id;
    drop(store.reserve(id, &FakeRng::default(), std::time::Duration::from_secs(30)));
    let spec = ProxySpec {
        box_id: id,
        box_handle: None,
        other_repos: Vec::new(),
        box_repo: None,
        rules: Vec::new(),
        user: UserId::new(3),
    };
    let files = ca::ca_dir(dir.path(), id);
    let (key_pem, cert_pem) = ca::generate(id).unwrap();
    ca::install(dir.path(), id, (key_pem.as_bytes(), &cert_pem)).unwrap();
    std::fs::write(files.join("spec.json"), serde_json::to_vec(&spec).unwrap()).unwrap();

    assert_eq!(import(&store, &vault, dir.path()), Vec::<String>::new());

    assert_eq!(store.proxy_spec(id).unwrap(), Some(spec));
    let kept = store.proxy_ca(id).unwrap().unwrap();
    assert_eq!(kept.cert_pem, cert_pem);
    let key = vault.open_box_ca(id, &kept.sealed_key).unwrap();
    assert_eq!(key.expose(), key_pem.as_bytes());
    assert_eq!(import(&store, &vault, dir.path()), Vec::<String>::new());
    assert_eq!(
        store.proxy_ca(id).unwrap().unwrap(),
        kept,
        "a second run changes nothing"
    );
}
