//! Which sleeps keep what the Box holds, only a settled agent's, and what a close then says of the
//! Box's caches.

use moor_core::domain::hands::HandsInc;

use super::*;

fn lost(w: &World, id: BoxId) -> bool {
    let store = w.cx.store().unwrap();
    store
        .newest_event_of(id, "caches_not_saved")
        .unwrap()
        .is_some()
}

/// Working `a`, and clean idle `b` too big to wake, suspended for room.
fn suspended_for_room() -> (World, BoxId) {
    let (w, hv) = live_world(32_768);
    let (a, b) = (started(&w, "a"), started(&w, "b"));
    guest_memory(&w, b, CEILING);
    idle(&w, b);
    let (w, _) = agents_report(w, &[b], &[a]);
    holds(&w, b, CLEAN);
    tick(&w, hv, BELOW_RESERVE, 0, 0);
    assert_eq!(lifecycle(&w, b), Asleep);
    (w, b)
}

/// Two working Boxes, the one too big to wake suspended as a last resort.
fn suspended_at_work() -> (World, BoxId) {
    let (w, hv) = live_world(32_768);
    let (a, b) = (started(&w, "a"), started(&w, "b"));
    guest_memory(&w, b, CEILING);
    let (w, _) = agents_report(w, &[], &[a, b]);
    holds(&w, b, CLEAN);
    tick(&w, hv, CRITICAL, 0, 0);
    assert_eq!(lifecycle(&w, b), Pending);
    (w, b)
}

#[test]
fn an_idle_box_suspended_for_room_keeps_its_answer() {
    let (w, b) = suspended_for_room();
    assert_eq!(stored(&w, b), Some((0, 0)));
}

#[test]
fn a_last_resort_suspend_keeps_no_answer() {
    let (w, b) = suspended_at_work();
    assert_eq!(stored(&w, b), None);
}

#[test]
fn a_last_resort_suspend_with_runs_in_flight_keeps_no_answer() {
    let (w, hv) = live_world(32_768);
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
    let suspended = if lifecycle(&w, a) == Ready { b } else { a };
    assert_eq!(stored(&w, suspended), None);
}

#[test]
fn a_run_in_flight_keeps_no_answer_even_when_the_agent_is_quiet() {
    let (w, _) = live_world(32_768);
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
    autosleep::sleep_parked(&w.cx, store, &row, &handle, settled)
        .unwrap()
        .unwrap();
    assert_eq!(stored(&w, b), None);
}

#[test]
fn the_hard_ceiling_keeps_no_clean_answer() {
    let (w, _) = live_world(32_768);
    let b = started(&w, "b");
    let (w, _) = agents_report(w, &[], &[b]);
    holds(&w, b, CLEAN);
    let store = w.cx.store().unwrap();
    store
        .set_suspend_check((b, HandsInc::FIRST), Some(OLD_CLEAN))
        .unwrap();
    w.cx.clock().sleep(std::time::Duration::from_hours(12));
    autosleep::run(&w.cx, store, b, autosleep::AgentRead::Plain);
    assert_eq!(lifecycle(&w, b), Asleep);
    assert_eq!(stored(&w, b), None);
}

#[test]
fn an_idle_box_that_sleeps_on_its_timer_keeps_its_answer() {
    let (w, _) = live_world(32_768);
    let b = started(&w, "b");
    idle(&w, b);
    holds(&w, b, CLEAN);
    w.cx.clock().sleep(std::time::Duration::from_mins(6));
    autosleep::run(&w.cx, w.cx.store().unwrap(), b, autosleep::AgentRead::Plain);
    assert_eq!(lifecycle(&w, b), Asleep);
    assert_eq!(stored(&w, b), Some((0, 0)));
}

#[test]
fn asking_an_idle_box_to_sleep_keeps_its_answer_and_a_working_one_does_not() {
    let (w, _) = live_world(32_768);
    let (a, b) = (started(&w, "a"), started(&w, "b"));
    let (w, _) = agents_report(w, &[b], &[a]);
    idle(&w, b);
    holds(&w, a, CLEAN);
    holds(&w, b, CLEAN);
    for id in [a, b] {
        let box_name = w.cx.store().unwrap().box_row(id).unwrap().unwrap().name;
        crate::ops::sleep::run(&w.cx, &moor_core::BoxReq { box_name }).unwrap();
    }
    assert_eq!(stored(&w, a), None);
    assert_eq!(stored(&w, b), Some((0, 0)));
}

#[test]
fn an_unwoken_close_reports_the_caches_not_saved() {
    let (w, b) = suspended_for_room();
    let before = calls(&w, "wake");
    close_box(&w, b, false).unwrap();
    assert_eq!(calls(&w, "wake"), before);
    assert_eq!(state(&w, b), Some(HandsState::Deleted));
    assert!(lost(&w, b));
}

#[test]
fn a_forced_close_of_a_box_suspended_for_room_reports_the_caches_not_saved() {
    let (w, b) = suspended_for_room();
    close_box(&w, b, true).unwrap();
    assert_eq!(lifecycle(&w, b), Closed);
    assert!(lost(&w, b));
}

#[test]
fn a_forced_close_of_a_box_suspended_at_work_reports_the_caches_not_saved() {
    let (w, b) = suspended_at_work();
    close_box(&w, b, true).unwrap();
    assert_eq!(lifecycle(&w, b), Closed);
    assert!(lost(&w, b));
}

#[test]
fn asking_an_idle_box_with_an_unread_agent_to_sleep_keeps_no_answer() {
    let (w, _) = live_world(32_768);
    let b = started(&w, "b");
    idle(&w, b);
    holds(&w, b, CLEAN);
    super::super::unread::unread(&w, b);
    let box_name = w.cx.store().unwrap().box_row(b).unwrap().unwrap().name;
    crate::ops::sleep::run(&w.cx, &moor_core::BoxReq { box_name }).unwrap();
    assert_eq!(lifecycle(&w, b), Asleep);
    assert_eq!(stored(&w, b), None);
}

#[test]
fn the_hard_ceiling_of_an_idle_box_read_as_unread_keeps_no_answer() {
    let (w, _) = live_world(32_768);
    let b = started(&w, "b");
    idle(&w, b);
    holds(&w, b, CLEAN);
    w.cx.clock().sleep(std::time::Duration::from_hours(12));
    let store = w.cx.store().unwrap();
    autosleep::run(&w.cx, store, b, autosleep::AgentRead::Unread);
    assert_eq!(lifecycle(&w, b), Asleep);
    assert_eq!(stored(&w, b), None);
}

#[test]
fn an_idle_box_with_an_unread_agent_suspended_for_room_keeps_no_answer() {
    let (w, hv) = live_world(32_768);
    let (a, b) = (started(&w, "a"), started(&w, "b"));
    idle(&w, b);
    let (w, _) = agents_report(w, &[], &[a]);
    holds(&w, b, CLEAN);
    super::super::unread::unread(&w, b);
    tick(&w, hv, CRITICAL, 0, 0);
    assert_eq!(lifecycle(&w, b), Pending, "an unread agent may be at work");
    assert_eq!(stored(&w, b), None);
}

#[test]
fn a_sleep_of_hands_replaced_before_it_began_never_reaches_the_driver() {
    let (w, _) = live_world(32_768);
    let b = started(&w, "b");
    idle(&w, b);
    holds(&w, b, CLEAN);
    let store = w.cx.store().unwrap();
    let row = store.box_row(b).unwrap().unwrap();
    let handle = autosleep::ready_handle(&row).unwrap();
    store.replace_hands(b);
    let before = calls(&w, "sleep");

    let slept = autosleep::sleep_parked(&w.cx, store, &row, &handle, true).unwrap();

    assert_eq!(slept, None);
    assert_eq!(calls(&w, "sleep"), before);
    assert_eq!(stored(&w, b), None);
}
