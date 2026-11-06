//! Whether a sleep lets the provider save the Box's caches, and what a close then says of them.

use moor_core::seam::driver::PausePublish;

use super::*;

/// A node whose agent reports that a pause can save caches.
fn saving_world() -> (World, i64) {
    let (w, hv) = live_world(32_768);
    w.driver.saves_on_pause();
    report(&w, hv, 32_768, None);
    (w, hv)
}

fn publishes(w: &World) -> Vec<PausePublish> {
    w.driver.pause_publishes()
}

fn decided(w: &World, id: BoxId) -> PausePublish {
    w.cx.store().unwrap().pause_publish(id).unwrap()
}

fn lost(w: &World, id: BoxId) -> bool {
    let store = w.cx.store().unwrap();
    store
        .newest_event_of(id, "caches_not_saved")
        .unwrap()
        .is_some()
}

/// Working `a`, and clean idle `b` too big to wake, suspended for room on a provider that saves.
fn suspended_for_room() -> (World, i64, BoxId) {
    let (w, hv) = saving_world();
    let (a, b) = (started(&w, "a"), started(&w, "b"));
    guest_memory(&w, b, CEILING);
    idle(&w, b);
    let (w, _) = agents_report(w, &[b], &[a]);
    holds(&w, b, CLEAN);
    tick(&w, hv, BELOW_RESERVE, 0, 0);
    assert_eq!(lifecycle(&w, b), Asleep);
    (w, hv, b)
}

/// Two working Boxes on a provider that saves, the one too big to wake suspended as a last resort.
fn suspended_at_work() -> (World, BoxId) {
    let (w, hv) = saving_world();
    let (a, b) = (started(&w, "a"), started(&w, "b"));
    guest_memory(&w, b, CEILING);
    let (w, _) = agents_report(w, &[], &[a, b]);
    holds(&w, b, CLEAN);
    tick(&w, hv, BELOW_RESERVE, 0, 0);
    assert_eq!(lifecycle(&w, b), Pending);
    (w, b)
}

#[test]
fn an_idle_box_suspended_for_room_lets_the_provider_save() {
    let (w, _, b) = suspended_for_room();
    assert_eq!(publishes(&w), [PausePublish::Allowed]);
    assert_eq!(decided(&w, b), PausePublish::Allowed);
    assert_eq!(stored(&w, b), Some((0, 0)));
}

#[test]
fn a_provider_that_cannot_save_is_never_asked_to() {
    let (w, _, _, b) = suspended_holding(CLEAN);
    assert_eq!(publishes(&w), [PausePublish::Denied]);
    assert_eq!(decided(&w, b), PausePublish::Denied);
    assert_eq!(
        stored(&w, b),
        Some((0, 0)),
        "the check does not depend on it"
    );
}

#[test]
fn a_last_resort_suspend_saves_nothing() {
    let (w, b) = suspended_at_work();
    assert_eq!(publishes(&w), [PausePublish::Denied]);
    assert_eq!(decided(&w, b), PausePublish::Denied);
    assert_eq!(stored(&w, b), None);
}

#[test]
fn a_last_resort_suspend_with_runs_in_flight_saves_nothing_and_keeps_no_answer() {
    let (w, hv) = saving_world();
    let (a, b) = (started(&w, "a"), started(&w, "b"));
    idle(&w, a);
    idle(&w, b);
    let (w, _) = agents_report(w, &[a, b], &[]);
    holds(&w, a, CLEAN);
    holds(&w, b, CLEAN);
    let store = w.cx.store().unwrap();
    let run = |box_id| crate::store::NewRun {
        box_id,
        argv: vec!["make".into()],
        record: None,
        sha: None,
        pid: 4242,
    };
    let _claims = (
        store.begin_run(run(a)).unwrap(),
        store.begin_run(run(b)).unwrap(),
    );
    tick(&w, hv, BELOW_RESERVE, 0, 0);
    assert_eq!(publishes(&w), [PausePublish::Denied]);
    let suspended = if lifecycle(&w, a) == Ready { b } else { a };
    assert_eq!(stored(&w, suspended), None);
}

#[test]
fn a_run_in_flight_denies_the_save_and_the_answer_even_when_the_agent_is_quiet() {
    let (w, _) = saving_world();
    let b = started(&w, "b");
    idle(&w, b);
    holds(&w, b, CLEAN);
    let store = w.cx.store().unwrap();
    let _claim = store
        .begin_run(crate::store::NewRun {
            box_id: b,
            argv: vec!["make".into()],
            record: None,
            sha: None,
            pid: 4242,
        })
        .unwrap();
    let row = store.box_row(b).unwrap().unwrap();
    let handle = autosleep::ready_handle(&row).unwrap();
    let settled = autosleep::settled(&w.cx, store, &row).unwrap();
    autosleep::sleep_parked(&w.cx, store, &row, &handle, settled).unwrap();
    assert_eq!(publishes(&w), [PausePublish::Denied]);
    assert_eq!(stored(&w, b), None);
}

#[test]
fn the_awake_cap_saves_nothing_and_keeps_no_clean_answer() {
    let (w, _) = saving_world();
    let b = started(&w, "b");
    let (w, _) = agents_report(w, &[], &[b]);
    holds(&w, b, CLEAN);
    let store = w.cx.store().unwrap();
    store.set_suspend_check(b, Some(OLD_CLEAN)).unwrap();
    w.cx.clock().sleep(std::time::Duration::from_mins(120));
    autosleep::run(&w.cx, store, b, autosleep::AgentRead::Plain);
    assert_eq!(lifecycle(&w, b), Asleep);
    assert_eq!(publishes(&w), [PausePublish::Denied]);
    assert_eq!(stored(&w, b), None);
}

#[test]
fn an_idle_box_that_sleeps_on_its_timer_lets_the_provider_save() {
    let (w, _) = saving_world();
    let b = started(&w, "b");
    idle(&w, b);
    holds(&w, b, CLEAN);
    w.cx.clock().sleep(std::time::Duration::from_mins(6));
    autosleep::run(&w.cx, w.cx.store().unwrap(), b, autosleep::AgentRead::Plain);
    assert_eq!(lifecycle(&w, b), Asleep);
    assert_eq!(publishes(&w), [PausePublish::Allowed]);
    assert_eq!(stored(&w, b), Some((0, 0)));
}

#[test]
fn asking_a_working_box_to_sleep_saves_nothing_and_an_idle_one_saves() {
    let (w, _) = saving_world();
    let (a, b) = (started(&w, "a"), started(&w, "b"));
    let (w, _) = agents_report(w, &[b], &[a]);
    idle(&w, b);
    for id in [a, b] {
        let box_name = w.cx.store().unwrap().box_row(id).unwrap().unwrap().name;
        crate::ops::sleep::run(&w.cx, &moor_core::BoxReq { box_name }).unwrap();
    }
    assert_eq!(publishes(&w), [PausePublish::Denied, PausePublish::Allowed]);
}

#[test]
fn a_wake_clears_the_decision() {
    let (w, hv, b) = suspended_for_room();
    assert_eq!(decided(&w, b), PausePublish::Allowed);
    guest_memory(&w, b, 1024);
    tick(&w, hv, 32_768, 0, 0);
    wake_up(&w, b).unwrap();
    assert_eq!(lifecycle(&w, b), Ready);
    assert_eq!(decided(&w, b), PausePublish::Denied);
}

#[test]
fn a_box_found_stopped_saved_nothing() {
    let (w, _) = live_world(32_768);
    let b = started(&w, "b");
    idle(&w, b);
    let store = w.cx.store().unwrap();
    store.set_pause_publish(b, PausePublish::Allowed).unwrap();
    let row = store.box_row(b).unwrap().unwrap();
    w.driver.stop_behind(&row.boxed.unwrap().handle.unwrap());
    crate::ops::common::reconcile::run(&w.cx);
    assert_eq!(state(&w, b), Some(BoxState::Asleep));
    assert_eq!(decided(&w, b), PausePublish::Denied);
}

#[test]
fn an_unwoken_close_of_a_box_the_provider_saved_reports_no_loss() {
    let (w, _, b) = suspended_for_room();
    let before = calls(&w, "wake");
    close_box(&w, b, false).unwrap();
    assert_eq!(calls(&w, "wake"), before);
    assert_eq!(lifecycle(&w, b), Closed);
    assert!(!lost(&w, b));
}

#[test]
fn a_forced_close_of_a_box_the_provider_saved_reports_no_loss() {
    let (w, _, b) = suspended_for_room();
    close_box(&w, b, true).unwrap();
    assert_eq!(lifecycle(&w, b), Closed);
    assert!(!lost(&w, b));
}

#[test]
fn an_unwoken_close_of_a_box_denied_the_save_reports_the_loss() {
    let (w, _, b) = suspended_for_room();
    let store = w.cx.store().unwrap();
    store.set_pause_publish(b, PausePublish::Denied).unwrap();
    close_box(&w, b, false).unwrap();
    assert_eq!(state(&w, b), Some(BoxState::Deleted));
    assert!(lost(&w, b));
}

#[test]
fn a_forced_close_of_a_box_suspended_at_work_reports_the_loss() {
    let (w, b) = suspended_at_work();
    close_box(&w, b, true).unwrap();
    assert_eq!(lifecycle(&w, b), Closed);
    assert!(lost(&w, b));
}

#[test]
fn asking_an_idle_box_with_an_unread_agent_to_sleep_saves_nothing_and_keeps_no_answer() {
    let (w, _) = saving_world();
    let b = started(&w, "b");
    idle(&w, b);
    holds(&w, b, CLEAN);
    super::super::unread::unread(&w, b);
    let box_name = w.cx.store().unwrap().box_row(b).unwrap().unwrap().name;
    crate::ops::sleep::run(&w.cx, &moor_core::BoxReq { box_name }).unwrap();
    assert_eq!(lifecycle(&w, b), Asleep);
    assert_eq!(publishes(&w), [PausePublish::Denied]);
    assert_eq!(decided(&w, b), PausePublish::Denied);
    assert_eq!(stored(&w, b), None);
}

#[test]
fn the_awake_cap_of_an_idle_box_read_as_unread_saves_nothing() {
    let (w, _) = saving_world();
    let b = started(&w, "b");
    idle(&w, b);
    holds(&w, b, CLEAN);
    w.cx.clock().sleep(std::time::Duration::from_mins(120));
    let store = w.cx.store().unwrap();
    autosleep::run(&w.cx, store, b, autosleep::AgentRead::Unread);
    assert_eq!(lifecycle(&w, b), Asleep);
    assert_eq!(publishes(&w), [PausePublish::Denied]);
    assert_eq!(stored(&w, b), None);
}

#[test]
fn an_idle_box_with_an_unread_agent_suspended_for_room_saves_nothing() {
    let (w, hv) = saving_world();
    let (a, b) = (started(&w, "a"), started(&w, "b"));
    idle(&w, b);
    let (w, _) = agents_report(w, &[], &[a]);
    holds(&w, b, CLEAN);
    super::super::unread::unread(&w, b);
    tick(&w, hv, BELOW_RESERVE, 0, 0);
    assert_eq!(publishes(&w), [PausePublish::Denied]);
}
