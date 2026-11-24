//! The place keeper gives each user on the `agent_places` list one unclaimed place, and never
//! starts another after a failure on its own.

use std::cell::Cell;

use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::decision::{Decision, NewBox, SpecFields};
use moor_core::domain::hands::{HandsInc, HandsInput};
use moor_core::domain::place::{BoxKind, Claim, PlaceState};
use moor_core::seam::forge::ForgeRepo;
use moor_core::{BoxId, CloseReq, ErrorKind};

use super::place::listed;
use super::{world, World};
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::close;
use crate::ops::place::{keep, Keeper};
use crate::ops::start::run;
use crate::store::BoxRow;
use crate::testing::box_name;
use crate::testing::cx::{new_idle_place, REQUESTER_USER};
use crate::testing::driver::FakeDriver;
use moor_core::domain::event::close_reason::CloseReason;

/// One pass of the keeper whose starts are the real start, as the listed requester.
fn pass(w: &World, keeper: &Keeper) -> Vec<String> {
    keep(&w.cx, keeper, &|_, _, req| run(&w.cx, req), &|| false).unwrap()
}

/// The open places, claimed or not, in order.
fn open_places(w: &World) -> Vec<BoxRow> {
    let open = w.cx.store().unwrap().open_boxes().unwrap();
    open.into_iter()
        .filter(|r| matches!(r.kind, BoxKind::Place(_)))
        .collect()
}

/// The one open place there is; the test fails when there is not exactly one.
fn the_place(w: &World) -> BoxRow {
    let mut places = open_places(w);
    assert_eq!(places.len(), 1, "{places:?}");
    places.remove(0)
}

/// The one line a pass said; the test fails when it said another number.
fn the_line(mut said: Vec<String>) -> String {
    assert_eq!(said.len(), 1, "{said:?}");
    said.remove(0)
}

/// An unclaimed place of the requester, still setting up.
fn setting_up(cx: &Cx, name: &str) -> BoxId {
    let store = cx.store().unwrap();
    let id = store.apply(new_idle_place(box_name(name))).unwrap().box_id;
    store
        .apply(Decision::new(id).lifecycle(BoxLifecycleInput::Admitted))
        .unwrap();
    id
}

/// An unclaimed place of the requester whose setup failed.
fn failed(cx: &Cx, name: &str) -> BoxId {
    let id = setting_up(cx, name);
    cx.store()
        .unwrap()
        .apply(Decision::new(id).lifecycle(BoxLifecycleInput::SetupFailed {
            cause: "no machine".to_owned(),
            which: None,
        }))
        .unwrap();
    id
}

fn closed_reason(w: &World, id: BoxId) -> String {
    let events = w.cx.store().unwrap().events(id, 100).unwrap();
    let closed = events.iter().find(|e| e.kind == "closed").unwrap();
    let reason = closed.payload.get("reason").and_then(|r| r.as_str());
    reason.unwrap().to_owned()
}

#[test]
fn a_roster_user_with_no_idle_place_gets_one_start() {
    let w = listed();
    assert!(pass(&w, &Keeper::default()).is_empty());
    let place = the_place(&w);
    assert_eq!(place.kind, BoxKind::Place(PlaceState::Idle));
    assert!(place.name.as_str().starts_with("place-"));
    assert_eq!(place.user, Some(REQUESTER_USER));
}

#[test]
fn a_second_pass_starts_none_while_the_place_stands_or_sets_up() {
    let w = listed();
    let keeper = Keeper::default();
    pass(&w, &keeper);
    let first = the_place(&w).id;
    pass(&w, &keeper);
    assert_eq!(the_place(&w).id, first);

    let w = listed();
    let coming = setting_up(&w.cx, "coming");
    pass(&w, &keeper);
    assert_eq!(the_place(&w).id, coming);
}

#[test]
fn a_restarted_controller_finds_the_place_and_starts_no_second() {
    let w = listed();
    pass(&w, &Keeper::default());
    let first = the_place(&w).id;
    pass(&w, &Keeper::default());
    assert_eq!(the_place(&w).id, first);
}

#[test]
fn a_lost_idle_place_is_closed_and_replaced() {
    let w = listed();
    let keeper = Keeper::default();
    pass(&w, &keeper);
    let lost = the_place(&w).id;
    w.cx.store()
        .unwrap()
        .apply(Decision::new(lost).hands_input(HandsInc::FIRST, HandsInput::DriverMissing))
        .unwrap();
    pass(&w, &keeper);
    assert_ne!(the_place(&w).id, lost);
    assert_eq!(closed_reason(&w, lost), "place_lost");
}

#[test]
fn an_idle_place_stopped_behind_moor_is_closed_and_replaced() {
    let w = listed();
    let keeper = Keeper::default();
    pass(&w, &keeper);
    let stopped = the_place(&w).id;
    w.cx.store()
        .unwrap()
        .apply(Decision::new(stopped).hands_input(HandsInc::FIRST, HandsInput::Sleep))
        .unwrap();
    pass(&w, &keeper);
    assert_ne!(the_place(&w).id, stopped);
    assert_eq!(closed_reason(&w, stopped), "place_lost");
}

#[test]
fn a_failed_place_start_is_not_retried_until_it_is_closed() {
    let w = listed();
    let keeper = Keeper::default();
    let id = failed(&w.cx, "broken");
    let said = the_line(pass(&w, &keeper));
    assert!(
        said.contains("broken") && said.contains("no machine"),
        "{said}"
    );
    for _ in 0..3 {
        assert!(pass(&w, &keeper).is_empty(), "said twice");
    }
    assert_eq!(the_place(&w).id, id);
    let req = CloseReq {
        target: id.into(),
        force: true,
    };
    close::run_with_reason(&w.cx, &req, CloseReason::requested("ann")).unwrap();
    pass(&w, &keeper);
    assert_ne!(the_place(&w).id, id);
}

#[test]
fn a_rejected_place_start_is_not_retried() {
    let w = listed();
    let keeper = Keeper::default();
    let store = w.cx.store().unwrap();
    let id = store
        .apply(new_idle_place(box_name("nope")))
        .unwrap()
        .box_id;
    store
        .apply(Decision::new(id).lifecycle(BoxLifecycleInput::Rejected {
            reason: "no credential".to_owned(),
        }))
        .unwrap();
    let said = the_line(pass(&w, &keeper));
    assert!(said.contains("nope") && said.contains("rejected"), "{said}");
    assert!(pass(&w, &keeper).is_empty());
    assert!(open_places(&w).is_empty());
}

#[test]
fn a_start_that_leaves_no_box_is_not_repeated_by_the_next_pass() {
    let w = listed();
    let keeper = Keeper::default();
    let starts = Cell::new(0);
    let refuse = |_: _, _: &_, _: &_| -> Result<_, CommandError> {
        starts.set(starts.get() + 1);
        Err(CommandError::new(ErrorKind::Failed, "no node is online"))
    };
    let said = the_line(keep(&w.cx, &keeper, &refuse, &|| false).unwrap());
    assert!(said.contains("no node is online"), "{said}");
    for _ in 0..3 {
        assert!(keep(&w.cx, &keeper, &refuse, &|| false).unwrap().is_empty());
    }
    assert_eq!(starts.get(), 1);
    // A restarted controller has no memory, which is the operator's retry.
    keep(&w.cx, &Keeper::default(), &refuse, &|| false).unwrap();
    assert_eq!(starts.get(), 2);
}

#[test]
fn a_claimed_place_does_not_count_and_the_next_one_is_started() {
    let w = listed();
    let keeper = Keeper::default();
    let claim = Claim {
        repo: ForgeRepo::new("o", "n"),
        also: Vec::new(),
        prompt: "where".to_owned(),
        at: 5,
        served: None,
        steered: None,
    };
    let taken = NewBox {
        name: box_name("taken"),
        repo: None,
        spec: SpecFields {
            kind: BoxKind::Place(PlaceState::Claimed(claim)),
            ..SpecFields::default()
        },
    };
    let store = w.cx.store().unwrap();
    store
        .apply(Decision::insert(taken).requester(REQUESTER_USER))
        .unwrap();
    pass(&w, &keeper);
    let idle = open_places(&w)
        .into_iter()
        .filter(|r| r.kind.is_idle_place());
    assert_eq!(idle.count(), 1);
}

#[test]
fn a_user_off_the_roster_gets_none() {
    let mut w = world(FakeDriver::new());
    w.cx =
        w.cx.with_env(|e| e.with_agent_place_users(["bob@example.com".to_owned()].into()));
    let starts = Cell::new(0);
    let count = |_: _, _: &_, _: &_| -> Result<_, CommandError> {
        starts.set(starts.get() + 1);
        Err(CommandError::new(ErrorKind::Failed, "unreachable"))
    };
    let said = keep(&w.cx, &Keeper::default(), &count, &|| false).unwrap();
    assert!(said.is_empty() && starts.get() == 0 && open_places(&w).is_empty());
}

#[test]
fn draining_stops_the_pass() {
    let w = listed();
    keep(
        &w.cx,
        &Keeper::default(),
        &|_, _, req| run(&w.cx, req),
        &|| true,
    )
    .unwrap();
    assert!(open_places(&w).is_empty());
}

#[test]
fn a_work_box_of_the_user_is_not_a_place() {
    let w = listed();
    run(&w.cx, &super::req("work")).unwrap();
    assert!(open_places(&w).is_empty());
    pass(&w, &Keeper::default());
    assert_eq!(open_places(&w).len(), 1);
}
