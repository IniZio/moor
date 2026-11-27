use super::*;
use moor_core::domain::place::{BoxKind, Claim, PlaceState};
use moor_core::seam::forge::ForgeRepo;

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

#[test]
fn a_place_sends_its_events_like_any_task() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = open(dir.path());
    store
        .enroll(&moor_core::user::Identity::local(1000), None)
        .expect("enroll");
    let claim = Claim {
        repo: ForgeRepo::new("o", "n"),
        also: Vec::new(),
        prompt: "where".to_owned(),
        at: 5,
        served: None,
        steered: None,
    };
    let new = NewBox {
        name: BoxName::parse_allow_bare("place").expect("name"),
        repo: None,
        spec: SpecFields {
            kind: BoxKind::Place(PlaceState::Claimed(claim)),
            ..SpecFields::default()
        },
    };
    let place = store
        .apply(Decision::insert(new).requester(UserId::new(1)))
        .expect("insert place")
        .box_id;
    append(&store, place, 1);
    let got = read_once(&dir, 0, &WatchFilter::default());
    assert_eq!(event_boxes(&got), ["place"], "{got:?}");
}
