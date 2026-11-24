//! `show` lists the Boxes of tasks: an idle place is none, and a claimed place is one.

use std::sync::Arc;

use moor_core::domain::decision::{Decision, NewBox, SpecFields};
use moor_core::domain::place::{BoxKind, Claim, PlaceState};
use moor_core::seam::forge::ForgeRepo;

use super::super::run;
use super::{add, cx, req};
use crate::testing::cx::{new_idle_place, REQUESTER_USER};
use crate::testing::driver::FakeDriver;

/// The names `show` lists, in order.
fn listed(cx: &crate::cx::Cx) -> Vec<String> {
    let shown = run(cx, &req(None, None)).unwrap();
    shown
        .boxes
        .iter()
        .flatten()
        .map(|b| b.name.clone())
        .collect()
}

#[test]
fn an_idle_place_is_not_listed_and_a_work_box_is() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path(), &Arc::new(FakeDriver::new()));
    add(&cx, "work");
    let store = cx.store().unwrap();
    store
        .apply(new_idle_place(crate::testing::box_name("place")))
        .unwrap();
    assert_eq!(listed(&cx), ["work"]);
}

#[test]
fn a_claimed_place_is_listed_like_any_task() {
    let dir = tempfile::tempdir().unwrap();
    let cx = cx(dir.path(), &Arc::new(FakeDriver::new()));
    let store = cx.store().unwrap();
    let claim = Claim {
        repo: ForgeRepo::new("o", "n"),
        also: Vec::new(),
        prompt: "where".to_owned(),
        at: 5,
        served: None,
        steered: None,
    };
    let new = NewBox {
        name: crate::testing::box_name("taken"),
        repo: None,
        spec: SpecFields {
            kind: BoxKind::Place(PlaceState::Claimed(claim)),
            ..SpecFields::default()
        },
    };
    store
        .apply(Decision::insert(new).requester(REQUESTER_USER))
        .unwrap();
    assert_eq!(listed(&cx), ["taken"]);
}
