use super::*;
use moor_core::domain::place::{BoxKind, Claim, PlaceState};
use moor_core::seam::forge::ForgeRepo;

/// An unclaimed place `name`.
fn add_place(store: &Store, name: &str) -> BoxId {
    let new = NewBox {
        name: BoxName::parse_allow_bare(name).expect("name"),
        repo: None,
        spec: SpecFields {
            kind: BoxKind::Place(PlaceState::Idle),
            ..SpecFields::default()
        },
    };
    let decision = Decision::insert(new).requester(UserId::new(1));
    store.apply(decision).expect("insert place").box_id
}

/// The Boxes named by the event frames in `got`, in order and without repeats.
fn event_boxes(got: &[Value]) -> Vec<String> {
    let mut boxes: Vec<String> = Vec::new();
    for frame in got.iter().filter(|f| f["frame"] == "event") {
        let name = frame["box"].as_str().expect("box").to_owned();
        if !boxes.contains(&name) {
            boxes.push(name);
        }
    }
    boxes
}

/// A store holding user 1, whom the places act for.
fn open_with_user(dir: &Path) -> Store {
    let store = open(dir);
    store
        .enroll(&moor_core::user::Identity::local(1000), None)
        .expect("enroll");
    store
}

#[test]
fn an_idle_place_sends_no_stream_event_and_a_work_box_does() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = open_with_user(dir.path());
    let work = add(&store, "work");
    append(&store, work, 1);
    let place = add_place(&store, "place");
    append(&store, place, 2);
    let got = read_once(&dir, 0, &WatchFilter::default());
    assert_eq!(event_boxes(&got), ["work"], "{got:?}");
}

#[test]
fn a_claimed_place_sends_its_events_like_any_task() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = open_with_user(dir.path());
    let place = add_place(&store, "place");
    append(&store, place, 1);
    let claim = Claim {
        repo: ForgeRepo::new("o", "n"),
        also: Vec::new(),
        prompt: "where".to_owned(),
        at: 5,
        served: None,
        steered: None,
    };
    store
        .put_claim(place, UserId::new(1), &claim)
        .expect("claim");
    let got = read_once(&dir, 0, &WatchFilter::default());
    assert_eq!(event_boxes(&got), ["place"], "{got:?}");
}
