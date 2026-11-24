//! `start --place` makes a repo-less Box that waits for a task, for a user on the controller's
//! `agent_places` list and no one else.

use std::collections::BTreeSet;

use moor_core::domain::place::{BoxKind, PlaceState};
use moor_core::user::Email;
use moor_core::{ErrorKind, StartReq};

use super::{repo, req, world, World};
use crate::ops::start::run;
use crate::testing::cx::requester;
use crate::testing::driver::FakeDriver;

/// A start of place `name`.
pub(super) fn place_req(name: &str) -> StartReq {
    StartReq {
        place: true,
        ..req(name)
    }
}

/// A world whose requester is on the `agent_places` list as `ann@example.com`.
pub(super) fn listed() -> World {
    let mut w = world(FakeDriver::new());
    w.cx.store()
        .unwrap()
        .enroll(
            &requester(),
            Some(&Email::parse("Ann@Example.com").unwrap()),
        )
        .unwrap();
    let roster = BTreeSet::from(["ann@example.com".to_owned()]);
    w.cx = w.cx.with_env(|e| e.with_agent_place_users(roster));
    w
}

#[test]
fn a_place_starts_with_no_repo_and_no_prompt() {
    let w = listed();
    let v = run(&w.cx, &place_req("p")).unwrap();
    let row =
        w.cx.store()
            .unwrap()
            .box_row(moor_core::BoxId::new(v.boxed.id));
    let row = row.unwrap().unwrap();
    assert_eq!(row.kind, BoxKind::Place(PlaceState::Idle));
    assert!(row.repo.is_none() && row.prompt.is_none() && row.issue.is_none());
    assert!(row.hands.is_some_and(|h| h.handle.is_some()));
}

#[test]
fn a_start_that_asks_for_no_place_makes_a_work_box() {
    let w = listed();
    let v = run(&w.cx, &req("w")).unwrap();
    let row =
        w.cx.store()
            .unwrap()
            .box_row(moor_core::BoxId::new(v.boxed.id));
    assert_eq!(row.unwrap().unwrap().kind, BoxKind::Work);
}

#[test]
fn a_user_off_the_list_is_refused_before_any_box() {
    let w = world(FakeDriver::new());
    let err = run(&w.cx, &place_req("p")).unwrap_err();
    assert_eq!(err.kind, ErrorKind::Refused, "{}", err.error);
    assert!(w.cx.store().unwrap().open_boxes().unwrap().is_empty());
    assert!(w.driver.calls().is_empty());
}

#[test]
fn a_place_takes_no_repo_and_no_prompt() {
    let w = listed();
    let with_repo = StartReq {
        repo: Some(repo()),
        ..place_req("p")
    };
    let with_prompt = StartReq {
        prompt: Some("hello".to_owned()),
        ..place_req("p")
    };
    for asked in [with_repo, with_prompt] {
        let err = run(&w.cx, &asked).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Usage, "{}", err.error);
    }
    assert!(w.cx.store().unwrap().open_boxes().unwrap().is_empty());
}
