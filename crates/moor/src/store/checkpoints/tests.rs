#![allow(clippy::indexing_slicing, reason = "test assertions")]

use std::sync::Arc;
use std::time::Duration;

use moor_core::domain::layers::{chain, LayerKey};
use moor_core::seam::clock::{Clock as _, Ts};
use moor_core::seam::driver::Checkpoint;
use moor_core::ProviderId;

use super::evict::EvictReason;
use super::Lineage;
use crate::store::Store;
use crate::testing::clock::FakeClock;
use crate::testing::cx::REQUESTER_USER;

fn made(id: &str) -> Checkpoint {
    Checkpoint {
        id: id.to_owned(),
        bytes: Some(7),
    }
}

fn of(repo: &str, keys: &[LayerKey]) -> Lineage {
    Lineage {
        user: REQUESTER_USER,
        repo: repo.to_owned(),
        chain: keys[0].clone(),
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
        .record_checkpoint(&local, &keys[0], &of("o/n", &keys), &made("cp-1"))
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
    let keys = chain(REQUESTER_USER, "o/n", "abc", &[]);
    let key = &keys[0];
    let local = ProviderId::new("local");
    store
        .record_checkpoint(&local, key, &of("o/n", &keys), &made("cp-1"))
        .unwrap();
    store
        .record_checkpoint(&local, key, &of("o/n", &keys), &made("cp-2"))
        .unwrap();
    assert_eq!(
        store.checkpoint(&local, key).unwrap().unwrap().checkpoint,
        "cp-2"
    );
}

#[test]
fn checkpoints_go_superseded_chains_first_then_least_recently_used() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    let store = Store::open(&dir.path().join("moor.db"), clock.clone()).unwrap();
    let local = ProviderId::new("local");
    let old = chain(REQUESTER_USER, "o/n", "aaa", &["a".to_owned()]);
    let other = chain(REQUESTER_USER, "o/other", "aaa", &[]);
    let new = chain(REQUESTER_USER, "o/n", "bbb", &[]);
    let save = |keys: &[LayerKey], layer: usize, repo: &str, id: &str| {
        clock.sleep(Duration::from_secs(1));
        store
            .record_checkpoint(&local, &keys[layer], &of(repo, keys), &made(id))
            .unwrap();
    };
    save(&other, 0, "o/other", "other-clone");
    save(&old, 0, "o/n", "old-clone");
    save(&old, 1, "o/n", "old-line");
    save(&new, 0, "o/n", "new-clone");
    clock.sleep(Duration::from_secs(1));
    assert!(store.use_checkpoint(&local, &old[1]).unwrap());
    let order: Vec<(String, EvictReason)> = store
        .evictable(&local)
        .unwrap()
        .into_iter()
        .map(|e| (e.checkpoint, e.reason))
        .collect();
    let (s, lru) = (EvictReason::Superseded, EvictReason::LeastRecentlyUsed);
    assert_eq!(
        order,
        [
            ("old-clone".to_owned(), s),
            ("old-line".to_owned(), s),
            ("other-clone".to_owned(), lru),
            ("new-clone".to_owned(), lru),
        ],
        "an older commit's chain of the same repo goes first, then the least recently used"
    );
    assert!(store
        .evictable(&ProviderId::new("other"))
        .unwrap()
        .is_empty());
}
