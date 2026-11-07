#![allow(clippy::indexing_slicing, reason = "test assertions")]

use std::sync::Arc;
use std::time::Duration;

use moor_core::domain::layers::{LayerIdentity, LayerKey};
use moor_core::seam::clock::{Clock as _, Ts};
use moor_core::seam::driver::Checkpoint;
use moor_core::user::UserId;
use moor_core::ProviderId;

use moor_core::BoxPath;
use moor_schema::tables::SetupCheckpoints as Checkpoints;
use sea_query::{Expr, Query};

use super::evict::{EvictReason, Evictable};
use super::{Lineage, Place};
use crate::store::db::DbExt as _;
use crate::testing::clock::FakeClock;
use crate::testing::cx::REQUESTER_USER;

/// The keys of a chain pinned to `base`, so each base makes a chain of its own.
fn chain(user: UserId, repo: &str, base: &str, cache: &str, lines: &[String]) -> Vec<LayerKey> {
    let identity = LayerIdentity {
        user,
        repo,
        cache,
        class: "",
        pinned_base: Some(base),
    };
    moor_core::domain::layers::chain(&identity, lines)
}

fn made(id: &str) -> Checkpoint {
    Checkpoint {
        id: id.to_owned(),
        bytes: Some(7),
        parent: None,
    }
}

fn over(id: &str, parent: &str) -> Checkpoint {
    Checkpoint {
        parent: Some(parent.to_owned()),
        ..made(id)
    }
}

fn ids(gone: Vec<Evictable>) -> Vec<String> {
    gone.into_iter().map(|e| e.checkpoint).collect()
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
    let store = crate::store::db::testing::open_store(&dir.path().join("moor.db"), clock.clone());
    let keys = chain(REQUESTER_USER, "o/n", "abc", "", &["a".to_owned()]);
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
    let store = crate::store::db::testing::open_store(&dir.path().join("moor.db"), clock);
    let keys = chain(REQUESTER_USER, "o/n", "abc", "", &[]);
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
    let store = crate::store::db::testing::open_store(&dir.path().join("moor.db"), clock.clone());
    let local = ProviderId::new("local");
    let old = chain(REQUESTER_USER, "o/n", "aaa", "", &["a".to_owned()]);
    let other = chain(REQUESTER_USER, "o/other", "aaa", "", &[]);
    let new = chain(REQUESTER_USER, "o/n", "bbb", "", &[]);
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

#[test]
fn a_checkpoint_goes_only_once_none_is_a_delta_over_it() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    let store = crate::store::db::testing::open_store(&dir.path().join("moor.db"), clock.clone());
    let local = ProviderId::new("local");
    let keys = chain(
        REQUESTER_USER,
        "o/n",
        "aaa",
        "",
        &["a".to_owned(), "b".to_owned()],
    );
    let lineage = of("o/n", &keys);
    store
        .record_checkpoint(&local, &keys[0], &lineage, &made("clone"))
        .unwrap();
    clock.sleep(Duration::from_secs(1));
    store
        .record_checkpoint(&local, &keys[1], &lineage, &over("line-a", "clone"))
        .unwrap();
    clock.sleep(Duration::from_secs(1));
    store
        .record_checkpoint(&local, &keys[2], &lineage, &over("line-b", "line-a"))
        .unwrap();
    for (left, next) in [
        (["line-b"].as_slice(), "line-b"),
        (["line-a"].as_slice(), "line-a"),
        (["clone"].as_slice(), "clone"),
    ] {
        let evictable = store.evictable(&local).unwrap();
        assert_eq!(
            ids(evictable.clone()),
            left,
            "only the deepest delta may go"
        );
        assert!(store.claim_checkpoint(&local, &evictable[0]).unwrap());
        assert!(store.forget_checkpoint(&local, next).unwrap());
    }
    assert!(store.evictable(&local).unwrap().is_empty());
}

#[test]
fn a_claimed_checkpoint_stays_tracked_but_unusable_until_forgotten() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    let store = crate::store::db::testing::open_store(&dir.path().join("moor.db"), clock);
    let local = ProviderId::new("local");
    let keys = chain(REQUESTER_USER, "o/n", "aaa", "", &[]);
    store
        .record_checkpoint(&local, &keys[0], &of("o/n", &keys), &made("cp-1"))
        .unwrap();
    let gone = store.evictable(&local).unwrap().remove(0);
    assert!(store.claim_checkpoint(&local, &gone).unwrap());
    assert!(
        !store.claim_checkpoint(&local, &gone).unwrap(),
        "claimed once"
    );
    assert!(store.evictable(&local).unwrap().is_empty());
    assert_eq!(store.checkpoint(&local, &keys[0]).unwrap(), None);
    assert!(!store.use_checkpoint(&local, &keys[0]).unwrap());
    assert_eq!(ids(store.evicting(&local).unwrap()), ["cp-1"]);
    assert!(store.forget_checkpoint(&local, "cp-1").unwrap());
    assert!(store.evicting(&local).unwrap().is_empty());
    assert!(store.evictable(&local).unwrap().is_empty());
}

#[test]
fn a_layer_saved_again_while_its_old_checkpoint_goes_tracks_both() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    let store = crate::store::db::testing::open_store(&dir.path().join("moor.db"), clock);
    let local = ProviderId::new("local");
    let keys = chain(REQUESTER_USER, "o/n", "aaa", "", &[]);
    let lineage = of("o/n", &keys);
    store
        .record_checkpoint(&local, &keys[0], &lineage, &made("cp-1"))
        .unwrap();
    let gone = store.evictable(&local).unwrap().remove(0);
    assert!(store.claim_checkpoint(&local, &gone).unwrap());
    store
        .record_checkpoint(&local, &keys[0], &lineage, &made("cp-2"))
        .unwrap();
    assert_eq!(ids(store.evicting(&local).unwrap()), ["cp-1"]);
    assert!(store.forget_checkpoint(&local, "cp-1").unwrap());
    assert_eq!(
        store
            .checkpoint(&local, &keys[0])
            .unwrap()
            .unwrap()
            .checkpoint,
        "cp-2"
    );
}

#[test]
fn an_unclaimed_checkpoint_is_used_again_and_not_forgotten() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    let store = crate::store::db::testing::open_store(&dir.path().join("moor.db"), clock);
    let local = ProviderId::new("local");
    let keys = chain(REQUESTER_USER, "o/n", "aaa", "", &[]);
    store
        .record_checkpoint(&local, &keys[0], &of("o/n", &keys), &made("cp-1"))
        .unwrap();
    let gone = store.evictable(&local).unwrap().remove(0);
    assert!(store.claim_checkpoint(&local, &gone).unwrap());
    store.unclaim_checkpoint(&local, "cp-1").unwrap();
    assert!(store.evicting(&local).unwrap().is_empty());
    assert_eq!(ids(store.evictable(&local).unwrap()), ["cp-1"]);
    assert!(store.use_checkpoint(&local, &keys[0]).unwrap());
    assert!(
        !store.forget_checkpoint(&local, "cp-1").unwrap(),
        "only a claimed checkpoint is forgotten"
    );
}

#[test]
fn a_row_from_before_the_base_column_gives_no_place_and_is_still_evictable() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1_000)));
    let store = crate::store::db::testing::open_store(&dir.path().join("moor.db"), clock);
    let keys = chain(REQUESTER_USER, "o/n", "abc", "", &[]);
    let local = ProviderId::new("local");
    store
        .record_checkpoint(&local, &keys[0], &of("o/n", &keys), &made("old"))
        .unwrap();
    let place = Place {
        workdir: BoxPath::restore("/work/old").unwrap(),
        setup_ms: 5,
        base: "abc".to_owned(),
    };
    store.place_checkpoint(&local, &keys[0], &place).unwrap();
    assert_eq!(
        store.checkpoint(&local, &keys[0]).unwrap().unwrap().place,
        Some(place)
    );
    store
        .writer()
        .run(
            Query::update()
                .table(Checkpoints::Table)
                .value(Checkpoints::Base, Expr::val(None::<String>)),
        )
        .unwrap();
    let row = store.checkpoint(&local, &keys[0]).unwrap().unwrap();
    assert_eq!(row.place, None, "no Box starts from it");
    assert_eq!(ids(store.evictable(&local).unwrap()), ["old"]);
}
