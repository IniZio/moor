//! What a controller restart finds in flight on a started Box, and how it settles it.

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::decision::{Decision, Effect, EffectChange, WantChange, Wanted};
use moor_core::domain::event::PostCloneStep;
use moor_core::BoxId;

use super::setup::start_req;
use super::{script_base_yaml, world};
use crate::ops::common::provision::interrupted::{cut_short, drop_wanted, fail_interrupted};
use crate::ops::common::provision::resume_setup;
use crate::ops::start::run;
use crate::testing::driver::FakeDriver;

#[test]
fn a_run_a_whole_relaunch_ran_beside_ends_as_interrupted_once_its_follower_is_gone() {
    let w = world(FakeDriver::new());
    script_base_yaml(&w, None);
    run(&w.cx, &start_req()).unwrap();
    let store = w.cx.store().unwrap();
    let id = BoxId::new(1);
    assert_eq!(store.box_row(id).unwrap().unwrap().effect, None);
    let (cut, claim) = store
        .begin_run(crate::store::NewRun {
            box_id: id,
            argv: vec!["sleep".to_owned(), "600".to_owned()],
            record: None,
            sha: None,
            pid: 4242,
        })
        .unwrap();
    assert_eq!(
        store.box_row(id).unwrap().unwrap().effect,
        Some(Effect::Run { run: cut })
    );
    {
        let _turn = w.cx.turn(id, crate::cx::owners::Cmd::Start).unwrap();
        crate::ops::common::step::timed(&w.cx, store, id, PostCloneStep::Launch, || {
            let mid = store.box_row(id).unwrap().unwrap().effect;
            assert_eq!(
                mid,
                Some(Effect::Run { run: cut }),
                "Next(Launch) replaced the run"
            );
            ((), true)
        });
        store.end_launching(id).unwrap();
    }
    assert_eq!(
        store.box_row(id).unwrap().unwrap().effect,
        Some(Effect::Run { run: cut }),
        "Finish(Launch) or end_launching cleared the run"
    );
    drop(claim);
    drop_wanted(store).unwrap();
    assert_eq!(cut_short(store).unwrap(), [id]);
    assert!(fail_interrupted(&w.cx, store).unwrap().is_empty());
    let ended = store.run(cut).unwrap().unwrap();
    assert!(ended.finished_at.is_some(), "run row left open");
    assert!(ended.interrupted, "run not ended as interrupted");
    assert_eq!(store.box_row(id).unwrap().unwrap().effect, None);
    assert!(cut_short(store).unwrap().is_empty());
}

#[test]
fn a_run_still_followed_at_a_restart_keeps_its_effect_and_stays_open() {
    let w = world(FakeDriver::new());
    script_base_yaml(&w, None);
    run(&w.cx, &start_req()).unwrap();
    let store = w.cx.store().unwrap();
    let id = BoxId::new(1);
    let (cut, claim) = store
        .begin_run(crate::store::NewRun {
            box_id: id,
            argv: vec!["make".to_owned()],
            record: None,
            sha: None,
            pid: 1,
        })
        .unwrap();
    assert!(fail_interrupted(&w.cx, store).unwrap().is_empty());
    let open = store.run(cut).unwrap().unwrap();
    assert!(open.finished_at.is_none());
    assert_eq!(
        store.box_row(id).unwrap().unwrap().effect,
        Some(Effect::Run { run: cut })
    );
    drop(claim);
}

#[test]
fn a_close_wanted_with_nothing_in_flight_is_finished_after_a_restart() {
    let w = world(FakeDriver::new());
    script_base_yaml(&w, None);
    run(&w.cx, &start_req()).unwrap();
    let store = w.cx.store().unwrap();
    let id = BoxId::new(1);
    let close = Wanted::Close {
        force: true,
        reason: None,
    };
    store
        .apply(Decision::new(id).want(WantChange::Raise(close)))
        .unwrap();
    assert_eq!(store.box_row(id).unwrap().unwrap().effect, None);
    assert_eq!(fail_interrupted(&w.cx, store).unwrap(), [id]);
    resume_setup(&w.cx, store, id).unwrap();
    let row = store.box_row(id).unwrap().unwrap();
    assert_eq!(row.lifecycle, BoxLifecycle::Closed);
    assert_eq!(row.wanted, None);
}

#[test]
fn an_acquire_left_on_ready_hands_ends_at_a_restart_with_nothing_to_resume() {
    let w = world(FakeDriver::new());
    script_base_yaml(&w, None);
    run(&w.cx, &start_req()).unwrap();
    let store = w.cx.store().unwrap();
    let id = BoxId::new(1);
    store
        .apply(Decision::new(id).effect(EffectChange::Next(Effect::Acquire)))
        .unwrap();
    assert!(fail_interrupted(&w.cx, store).unwrap().is_empty());
    let row = store.box_row(id).unwrap().unwrap();
    assert_eq!(row.effect, None);
    assert_eq!(row.lifecycle, BoxLifecycle::Ready);
    assert!(cut_short(store).unwrap().is_empty());
}
