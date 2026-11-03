#![allow(clippy::indexing_slicing, reason = "test assertions")]

use std::sync::Arc;
use std::time::Duration;

use moor_core::domain::layers::chain;
use moor_core::seam::clock::{Clock as _, Ts};
use moor_core::seam::driver::Checkpoint;
use moor_core::ProviderId;

use crate::store::Store;
use crate::testing::clock::FakeClock;
use crate::testing::cx::REQUESTER_USER;

fn made(id: &str) -> Checkpoint {
    Checkpoint {
        id: id.to_owned(),
        bytes: Some(7),
    }
}

#[test]
fn a_layer_s_checkpoint_is_found_by_provider_and_key_and_its_use_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    let store = Store::open(&dir.path().join("moor.db"), clock.clone()).unwrap();
    let keys = chain(REQUESTER_USER, "o/n", "abc", &["a".to_owned()]);
    let local = ProviderId::new("local");
    store
        .record_checkpoint(&local, &keys[0], "o/n", &made("cp-1"))
        .unwrap();
    let row = store.checkpoint(&local, &keys[0]).unwrap().unwrap();
    assert_eq!(
        (row.checkpoint.as_str(), row.repo.as_str()),
        ("cp-1", "o/n")
    );
    assert_eq!(row.bytes, Some(7));
    assert_eq!(store.checkpoint(&local, &keys[1]).unwrap(), None);
    assert_eq!(
        store
            .checkpoint(&ProviderId::new("other"), &keys[0])
            .unwrap(),
        None,
        "a checkpoint lives under its own provider"
    );
    clock.sleep(Duration::from_secs(5));
    assert!(store.use_checkpoint(&local, &keys[0]).unwrap());
    let used = store.checkpoint(&local, &keys[0]).unwrap().unwrap();
    assert_eq!(used.created_at, Ts::from_millis(1_000));
    assert_eq!(used.last_used_at, Ts::from_millis(6_000));
    assert!(!store.use_checkpoint(&local, &keys[1]).unwrap());
}

#[test]
fn a_layer_saved_again_replaces_its_checkpoint() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    let store = Store::open(&dir.path().join("moor.db"), clock).unwrap();
    let key = &chain(REQUESTER_USER, "o/n", "abc", &[])[0];
    let local = ProviderId::new("local");
    store
        .record_checkpoint(&local, key, "o/n", &made("cp-1"))
        .unwrap();
    store
        .record_checkpoint(&local, key, "o/n", &made("cp-2"))
        .unwrap();
    assert_eq!(
        store.checkpoint(&local, key).unwrap().unwrap().checkpoint,
        "cp-2"
    );
}
