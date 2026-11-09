//! A Box that lost its machine gets new hands, a row of their own, while the lost ones stay as
//! history and nothing late about them lands on the new ones.

use moor_core::box_key::BoxKey;
use moor_core::decide::reconcile::{reconcile, BoxFact};
use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::decision::Decision;
use moor_core::domain::hands::{HandsInc, HandsInput, HandsState};
use moor_core::domain::layers::LayerKey;
use moor_core::seam::proxy_host::ProxySpec;
use moor_core::{BoxHandle, BoxId, ProviderId, SteerReq};

use super::placement::{cached_world, planned_volumes};
use super::relaunch::{gone, started};
use super::{agentless_req, repo_req, world, World};
use crate::ops::start::run;
use crate::store::{Store, StoreError};
use crate::testing::box_name;
use crate::testing::cx::REQUESTER_USER;
use crate::testing::driver::FakeDriver;
use crate::testing::rng::FakeRng;

fn demo(w: &World) -> BoxId {
    let store = w.cx.store().unwrap();
    store
        .open_box_by_name(&box_name("demo"))
        .unwrap()
        .unwrap()
        .id
}

fn lose_hands(w: &World, id: BoxId) {
    w.cx.store()
        .unwrap()
        .apply(Decision::new(id).hands_input(HandsInc::FIRST, HandsInput::DriverMissing))
        .unwrap();
}

fn history(w: &World, id: BoxId) -> Vec<(i64, String, bool)> {
    w.cx.store().unwrap().hands_history(id)
}

fn lost_and_replaced() -> Vec<(i64, String, bool)> {
    vec![(1, "gone".to_owned(), true), (2, "ready".to_owned(), false)]
}

#[test]
fn a_start_after_lost_hands_acquires_new_hands_under_their_own_slug() {
    let w = cached_world(&[], true);
    planned_volumes(&w);
    let id = demo(&w);
    lose_hands(&w, id);

    run(&w.cx, &repo_req("demo")).unwrap();

    assert_eq!(history(&w, id), lost_and_replaced());
    let hands =
        w.cx.store()
            .unwrap()
            .box_row(id)
            .unwrap()
            .unwrap()
            .hands
            .unwrap();
    assert_eq!(hands.inc, HandsInc::new(2));
    assert_eq!(
        hands.handle,
        Some(BoxHandle::new("box-2")),
        "the acquire wrote inc 2"
    );
    assert!(hands.workdir.is_some(), "the setup wrote inc 2");
    let seats = w.driver.acquired_seats();
    let slugs: Vec<String> = seats
        .iter()
        .map(|s| {
            let s = s.as_ref().expect("a personal node seats its Boxes");
            BoxKey::new(id).pool_slug(&s.controller, s.inc)
        })
        .collect();
    let ctrl = &seats.first().unwrap().as_ref().unwrap().controller;
    let first = format!("moor-{}-{}", ctrl.as_str(), id.get());
    assert_eq!(slugs, [first.clone(), format!("{first}-h2")]);
}

/// Writes a Proxy spec and a layer pin about Box `id`'s hands `inc`, as that machine's setup does.
fn write_vm_facts(store: &Store, id: BoxId, inc: HandsInc) -> [Result<(), StoreError>; 2] {
    let spec = ProxySpec {
        box_id: id,
        box_handle: Some(BoxHandle::new(format!("box-{}", inc.get()))),
        box_repo: None,
        other_repos: Vec::new(),
        rules: Vec::new(),
        user: REQUESTER_USER,
        hands_inc: Some(inc.get()),
    };
    let key = LayerKey::restore(inc.get().to_string().repeat(64)).unwrap();
    let home = ProviderId::new("fake");
    [
        store.set_proxy_spec(id, &spec),
        store.pin_checkpoint((id, Some(inc)), &home, &key, 1),
    ]
}

#[test]
fn the_new_hands_write_over_facts_the_lost_ones_left_and_late_ones_are_refused() {
    let w = world(FakeDriver::new());
    run(&w.cx, &agentless_req("demo")).unwrap();
    let id = demo(&w);
    let store = w.cx.store().unwrap();
    store.reserve(id, &FakeRng::default()).unwrap();
    let old = HandsInc::FIRST;
    assert!(write_vm_facts(store, id, old).iter().all(Result::is_ok));
    lose_hands(&w, id);

    run(&w.cx, &agentless_req("demo")).unwrap();

    let new = store.box_row(id).unwrap().unwrap().hands_inc();
    assert_eq!(new, HandsInc::new(2));
    for wrote in write_vm_facts(store, id, new) {
        wrote.unwrap();
    }
    for late in write_vm_facts(store, id, old) {
        assert!(matches!(late, Err(StoreError::Stale { .. })), "{late:?}");
    }
    let spec = store.proxy_spec(id).unwrap().unwrap();
    assert_eq!(spec.hands_inc, Some(2), "the late spec changed nothing");
    assert!(matches!(
        store.sleep_machine(id, Some(old)),
        Err(StoreError::Stale { .. })
    ));
    store.sleep_machine(id, Some(new)).unwrap();
}

#[test]
fn a_late_verdict_on_lost_hands_leaves_their_replacement_alone() {
    let w = world(FakeDriver::new());
    run(&w.cx, &agentless_req("demo")).unwrap();
    let id = demo(&w);
    lose_hands(&w, id);
    run(&w.cx, &agentless_req("demo")).unwrap();
    let fact = BoxFact {
        box_id: id,
        inc: HandsInc::FIRST,
        state: HandsState::Ready,
        handle: Some(BoxHandle::new("box-1")),
    };
    let late = reconcile(&[fact], &[]).pop().expect("a Gone verdict");

    let err = w.cx.store().unwrap().apply(late).unwrap_err();

    assert!(matches!(err, StoreError::Stale { .. }), "{err:?}");
    assert_eq!(history(&w, id), lost_and_replaced());
}

#[test]
fn a_start_brings_back_an_asleep_box_whose_hands_were_lost() {
    let w = world(FakeDriver::new());
    run(&w.cx, &agentless_req("demo")).unwrap();
    let id = demo(&w);
    let store = w.cx.store().unwrap();
    store
        .apply(
            Decision::new(id)
                .lifecycle(BoxLifecycleInput::Sleep {
                    auto: false,
                    reason: None,
                })
                .hands_input(HandsInc::FIRST, HandsInput::Sleep),
        )
        .unwrap();
    lose_hands(&w, id);

    run(&w.cx, &agentless_req("demo")).unwrap();

    let row = store.box_row(id).unwrap().unwrap();
    assert_eq!(row.lifecycle, BoxLifecycle::Ready);
    assert_eq!(history(&w, id), lost_and_replaced());
    assert!(store.newest_event_of(id, "disk_lost").unwrap().is_some());
}

#[test]
fn a_steer_to_a_box_whose_hands_were_lost_is_queued_and_the_pass_brings_it_back() {
    let (w, engine, id) = started();
    lose_hands(&w, id);
    gone(&engine);
    let req = SteerReq {
        target: moor_core::request::BoxTarget::Id(id),
        text: Some("carry on".to_owned()),
        menu: None,
        interrupt: false,
    };

    let sent = crate::ops::steer::run(&w.cx, &req).unwrap();
    assert_eq!(sent.sent, "carry on");
    assert!(sent.steer.is_none());
    crate::ops::inbox_pass(
        &w.cx,
        w.cx.store().unwrap(),
        &|| false,
        &crate::ops::InboxBackoff::default(),
    )
    .unwrap();

    assert_eq!(history(&w, id), lost_and_replaced());
    assert_eq!(engine.launches().last().unwrap().handle.as_str(), "box-2");
}
