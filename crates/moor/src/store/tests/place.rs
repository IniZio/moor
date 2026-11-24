//! A Box's kind as the store gives it back: work unless a start made it a place, and a place
//! idle until a claim row says a task took it.

use moor_core::domain::decision::{Decision, NewBox, SpecFields};
use moor_core::domain::place::{BoxKind, Claim, PlaceState};
use moor_core::seam::forge::ForgeRepo;

use super::open;
use crate::store::Store;
use crate::store::StoreError;
use crate::testing::box_name;
use crate::testing::cx::{box_repo, new_box, new_idle_place, requester, REQUESTER_USER};

/// A store in `dir` that holds [`REQUESTER_USER`], whom each Box and claim here acts for.
fn store_of(dir: &std::path::Path) -> Store {
    let (store, _) = open(dir);
    store.enroll(&requester(), None).unwrap();
    store
}

/// A claim of `o/a` with `o/b` beside it.
fn claim() -> Claim {
    Claim {
        repo: ForgeRepo::new("o", "a"),
        also: vec![ForgeRepo::new("o", "b")],
        prompt: "where is it".to_owned(),
        at: 7,
        served: Some(8),
        steered: None,
    }
}

#[test]
fn a_box_is_work_unless_its_start_made_it_a_place() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_of(dir.path());
    let work = store.apply(new_box(box_name("w"))).unwrap().box_id;
    let place = store.apply(new_idle_place(box_name("p"))).unwrap().box_id;
    assert_eq!(store.box_row(work).unwrap().unwrap().kind, BoxKind::Work);
    assert_eq!(
        store.box_row(place).unwrap().unwrap().kind,
        BoxKind::Place(PlaceState::Idle)
    );
}

#[test]
fn a_box_reads_back_its_task() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_of(dir.path());
    let place = store.apply(new_idle_place(box_name("p"))).unwrap().box_id;
    let tied = NewBox {
        name: box_name("w"),
        repo: None,
        spec: SpecFields {
            task: Some(place),
            ..SpecFields::default()
        },
    };
    let tied = store
        .apply(Decision::insert(tied).requester(REQUESTER_USER))
        .unwrap()
        .box_id;
    let manual = store.apply(new_box(box_name("v"))).unwrap().box_id;
    assert_eq!(store.box_row(tied).unwrap().unwrap().task, Some(place));
    assert_eq!(store.box_row(manual).unwrap().unwrap().task, None);
    assert_eq!(store.box_row(place).unwrap().unwrap().task, None);
}

#[test]
fn a_task_box_names_its_place_in_its_ties() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_of(dir.path());
    let place = store.apply(new_idle_place(box_name("p"))).unwrap().box_id;
    let tied = NewBox {
        name: box_name("w"),
        repo: None,
        spec: SpecFields {
            task: Some(place),
            ..SpecFields::default()
        },
    };
    let tied = store
        .apply(Decision::insert(tied).requester(REQUESTER_USER))
        .unwrap()
        .box_id;
    let manual = store.apply(new_box(box_name("v"))).unwrap().box_id;
    let named = store.ties_of(&[place, tied, manual]).unwrap();
    assert_eq!(named.get(&tied).and_then(|t| t.task.as_deref()), Some("p"));
    assert!(!named.contains_key(&place) && !named.contains_key(&manual));
}

#[test]
fn a_claim_on_a_place_reads_back_with_the_box() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_of(dir.path());
    let id = store.apply(new_idle_place(box_name("p"))).unwrap().box_id;
    store.put_claim(id, REQUESTER_USER, &claim()).unwrap();
    let kind = store.box_row(id).unwrap().unwrap().kind;
    assert_eq!(kind, BoxKind::Place(PlaceState::Claimed(claim())));
    assert!(!kind.is_idle_place());
}

#[test]
fn a_place_started_with_its_claim_reads_back_claimed() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_of(dir.path());
    let new = NewBox {
        name: box_name("p"),
        repo: None,
        spec: SpecFields {
            kind: BoxKind::Place(PlaceState::Claimed(claim())),
            ..SpecFields::default()
        },
    };
    let id = store
        .apply(Decision::insert(new).requester(REQUESTER_USER))
        .unwrap()
        .box_id;
    let kind = store.box_row(id).unwrap().unwrap().kind;
    assert_eq!(kind, BoxKind::Place(PlaceState::Claimed(claim())));
}

#[test]
fn a_claim_row_on_a_work_box_is_invalid() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_of(dir.path());
    let id = store.apply(new_box(box_name("w"))).unwrap().box_id;
    store.put_claim(id, REQUESTER_USER, &claim()).unwrap();
    let err = store.box_row(id).unwrap_err();
    assert!(matches!(err, StoreError::Invalid(_)), "{err}");
}

#[test]
fn a_place_with_a_repo_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_of(dir.path());
    let new = NewBox {
        name: box_name("p"),
        repo: Some(box_repo()),
        spec: SpecFields {
            kind: BoxKind::Place(PlaceState::Idle),
            ..SpecFields::default()
        },
    };
    let err = store.apply(Decision::insert(new)).unwrap_err();
    assert!(matches!(err, StoreError::Invalid(_)), "{err}");
}

#[test]
fn the_kind_alone_reads_the_same_as_the_kind_of_the_row() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_of(dir.path());
    let work = store.apply(new_box(box_name("w"))).unwrap().box_id;
    let idle = store.apply(new_idle_place(box_name("i"))).unwrap().box_id;
    let taken = store.apply(new_idle_place(box_name("t"))).unwrap().box_id;
    store.put_claim(taken, REQUESTER_USER, &claim()).unwrap();
    for id in [work, idle, taken] {
        let row = store.box_row(id).unwrap().unwrap().kind;
        assert_eq!(store.box_kind(id).unwrap(), Some(row));
    }
    assert_eq!(store.box_kind(moor_core::BoxId::new(99)).unwrap(), None);
}
