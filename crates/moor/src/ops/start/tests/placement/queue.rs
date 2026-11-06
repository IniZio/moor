//! The one queue on a node: what lets a Box resume, and in what order Boxes leave it.

use moor_core::domain::box_state::BoxLifecycle::{self, Asleep, Pending, Ready};
use moor_core::domain::decision::{Decision, Field, Fields};
use moor_core::request::BoxHostMemory;
use moor_core::seam::driver::DriverError;

use super::pressure::{agents_report, live_world_on, tick, wake_up, BELOW_RESERVE};
use super::*;

const CEILING: u64 = 23 * 1024;
const REAL_USE: u64 = 6 * 1024;

/// What a report must say for `free` MiB above the 4 GiB reserve.
const fn with_free(free: u64) -> u64 {
    4096 + free
}

fn guest_memory(w: &World, id: BoxId, mib: u64) {
    apply(
        w,
        Decision::new(id).set(Fields {
            box_cpus: Field::Set(4),
            box_mem_mib: Field::Set(i64::try_from(mib).unwrap()),
            ..Fields::default()
        }),
    );
}

fn provider_says(w: &World, hv: i64, id: BoxId, mib: u64) {
    let store = w.cx.store().unwrap();
    store
        .set_box_memory(&[BoxHostMemory {
            machine_id: hv,
            controller: store.controller_id().unwrap(),
            box_id: id,
            host_memory_mib: 0,
            resume_memory_mib: Some(mib),
        }])
        .unwrap();
}

fn reason(w: &World, id: BoxId) -> String {
    let row = w.cx.store().unwrap().box_row(id).unwrap().unwrap();
    row.state_reason.unwrap_or_default()
}

fn states(w: &World, ids: &[BoxId]) -> Vec<BoxLifecycle> {
    ids.iter().map(|id| lifecycle(w, *id)).collect()
}

/// A Box with the given guest memory, asleep.
fn asleep(w: &World, name: &str, guest: u64) -> BoxId {
    let id = started(w, name);
    guest_memory(w, id, guest);
    put_to_sleep(w, id);
    id
}

#[test]
fn a_ceiling_far_above_real_use_resumes_beside_a_light_box_once_its_provider_says_so() {
    let (w, hv) = live_world(32_768);
    let big = asleep(&w, "big", CEILING);
    let light = started(&w, "light");
    let (w, _) = agents_report(w, &[], &[light]);
    tick(&w, hv, with_free(13 * 1024), 0, 0);
    assert!(wake_up(&w, big).is_err(), "the ceiling alone does not fit");
    assert_eq!(lifecycle(&w, big), Pending);
    assert_eq!(
        reason(&w, big),
        "waiting for capacity on hv: 13 GiB free, needs 23 GiB"
    );
    provider_says(&w, hv, big, REAL_USE);
    tick(&w, hv, with_free(13 * 1024), 0, 0);
    assert_eq!(lifecycle(&w, big), Ready);
}

#[test]
fn only_asleep_boxes_and_too_little_memory_leave_the_head_waiting_with_its_reason_and_place() {
    let (w, hv) = live_world(32_768);
    let first = asleep(&w, "first", CEILING);
    let second = asleep(&w, "second", 16 * 1024);
    tick(&w, hv, 8192, 0, 0);
    assert!(wake_up(&w, first).is_err());
    assert!(wake_up(&w, second).is_err());
    for _ in 0..3 {
        tick(&w, hv, 8192, 0, 0);
    }
    assert_eq!(states(&w, &[first, second]), [Pending, Pending]);
    assert_eq!(
        reason(&w, first),
        "waiting for capacity on hv: 4 GiB free, needs 23 GiB"
    );
    let place = |id: BoxId| {
        let row = w.cx.store().unwrap().box_row(id).unwrap().unwrap();
        let shown = crate::ops::show::run(
            &w.cx,
            &moor_core::ShowReq {
                target: Some(row.name.into()),
                events: None,
                agent: false,
                reports: false,
                artifacts: false,
                pane: false,
            },
        )
        .unwrap();
        let held = shown.boxed.unwrap().held_machine.unwrap();
        (held.name, held.position)
    };
    assert_eq!(place(first), ("hv".to_owned(), Some(1)));
    assert_eq!(place(second), ("hv".to_owned(), Some(2)));
    tick(&w, hv, 32_768, 0, 0);
    assert_eq!(states(&w, &[first, second]), [Ready, Pending]);
    tick(&w, hv, 32_768, 0, 0);
    assert_eq!(states(&w, &[first, second]), [Ready, Ready]);
}

#[test]
fn a_small_box_behind_a_big_one_waits_its_turn_even_when_it_would_fit() {
    let (w, hv) = live_world(32_768);
    let big = asleep(&w, "big", CEILING);
    let small = asleep(&w, "small", 1024);
    let up = started(&w, "up");
    let (w, _) = agents_report(w, &[], &[up]);
    tick(&w, hv, with_free(8192), 0, 0);
    assert!(wake_up(&w, big).is_err());
    assert!(wake_up(&w, small).is_err(), "room, but it is behind big");
    assert_eq!(reason(&w, small), "hv is making room; 1 Boxes ahead");
    tick(&w, hv, with_free(8192), 0, 0);
    assert_eq!(states(&w, &[big, small]), [Pending, Pending], "no jump");
    tick(&w, hv, with_free(CEILING), 0, 0);
    assert_eq!(states(&w, &[big, small]), [Ready, Pending]);
    tick(&w, hv, with_free(CEILING), 0, 0);
    assert_eq!(states(&w, &[big, small]), [Ready, Ready]);
}

#[test]
fn a_new_start_with_room_goes_behind_a_resume_that_waits() {
    let (w, hv) = live_world(32_768);
    let big = asleep(&w, "big", CEILING);
    let up = started(&w, "up");
    let (w, _) = agents_report(w, &[], &[up]);
    tick(&w, hv, with_free(8192), 0, 0);
    assert!(wake_up(&w, big).is_err());
    let (fresh, why) = waits(&w, "fresh");
    assert_eq!(why, "hv is making room; 1 Boxes ahead");
    tick(&w, hv, with_free(CEILING), 0, 0);
    assert_eq!(states(&w, &[big, fresh]), [Ready, Pending]);
    tick(&w, hv, with_free(CEILING), 0, 0);
    assert_eq!(states(&w, &[big, fresh]), [Ready, Ready]);
}

#[test]
fn a_resume_goes_ahead_of_an_older_start_in_the_queue() {
    let (w, hv) = live_world(SHORT);
    let (older, _) = waits(&w, "older");
    let other = enrolled(&w, "other", Some(true), None);
    report(&w, other, 32_768, None);
    let mut on_other = req("resume");
    on_other.node = Some("other".to_owned());
    let resume = BoxId::new(start::run(&w.cx, &on_other).unwrap().boxed.id);
    put_to_sleep(&w, resume);
    let store = w.cx.store().unwrap();
    let hv_name = moor_core::name::NodeName::parse("hv").unwrap();
    let spec = moor_core::decide::schedule::BoxSpec {
        labels: Vec::new(),
        node: Some(hv_name.clone()),
    };
    store.set_box_spec(resume, &spec).unwrap();
    report(&w, hv, SHORT, None);
    assert!(wake_up(&w, resume).is_err());
    assert_eq!(store.pending_on(&hv_name).unwrap(), [resume, older]);
    report(&w, hv, 32_768, None);
    assert_eq!(states(&w, &[resume, older]), [Ready, Pending]);
}

#[test]
fn a_head_that_fails_on_its_own_account_holds_nothing_back() {
    let (w, hv) = live_world_on(
        FakeDriver::new().fail_wake(DriverError::Timeout(std::time::Duration::from_secs(1))),
    );
    let first = asleep(&w, "first", 8192);
    let second = asleep(&w, "second", 8192);
    tick(&w, hv, SHORT, 0, 0);
    assert!(wake_up(&w, first).is_err());
    assert!(wake_up(&w, second).is_err());
    tick(&w, hv, 32_768, 0, 0);
    assert_eq!(states(&w, &[first, second]), [Pending, Ready]);
}

#[test]
fn a_second_wake_waits_for_the_report_that_counts_the_first() {
    let (w, hv) = live_world(32_768);
    let (a, b) = (asleep(&w, "a", 1024), asleep(&w, "b", 1024));
    tick(&w, hv, 32_768, 0, 0);
    w.cx.clock().sleep(std::time::Duration::from_secs(2));
    wake_up(&w, a).unwrap();
    w.cx.clock().sleep(std::time::Duration::from_secs(2));
    assert!(wake_up(&w, b).is_err());
    assert_eq!(reason(&w, b), "waiting for the next memory report from hv");
    tick(&w, hv, 32_768, 0, 0);
    assert_eq!(lifecycle(&w, b), Ready);
}

#[test]
fn a_box_that_just_resumed_is_not_picked_again_until_it_settles() {
    let (w, hv) = live_world(32_768);
    let (a, b, newest) = (started(&w, "a"), started(&w, "b"), started(&w, "newest"));
    let (w, _) = agents_report(w, &[], &[a, b, newest]);
    tick(&w, hv, BELOW_RESERVE, 0, 0);
    assert_eq!(lifecycle(&w, newest), Pending);
    tick(&w, hv, 32_768, 0, 0);
    assert_eq!(lifecycle(&w, newest), Ready, "resumed");
    tick(&w, hv, BELOW_RESERVE, 0, 0);
    assert_eq!(
        states(&w, &[a, b, newest]),
        [Ready, Ready, Ready],
        "cooldown"
    );
    tick(&w, hv, BELOW_RESERVE, 0, 0);
    assert_eq!(states(&w, &[a, b, newest]), [Ready, Pending, Ready]);
}

#[test]
fn a_box_whose_machine_vanished_while_it_waited_gets_a_new_one_only_when_there_is_room() {
    let (w, hv) = live_world(32_768);
    let (a, b) = (started(&w, "a"), started(&w, "b"));
    let (w, _) = agents_report(w, &[], &[a, b]);
    tick(&w, hv, BELOW_RESERVE, 0, 0);
    assert_eq!(lifecycle(&w, b), Pending);
    let store = w.cx.store().unwrap();
    let row = store.box_row(b).unwrap().unwrap();
    w.driver.forget(&row.boxed.unwrap().handle.unwrap());
    crate::ops::common::reconcile::run(&w.cx);
    tick(&w, hv, BELOW_RESERVE, 0, 0);
    assert_eq!(lifecycle(&w, b), Pending, "no room yet");
    assert!(store.newest_event_of(b, "disk_lost").unwrap().is_none());
    tick(&w, hv, 32_768, 0, 0);
    assert_eq!(lifecycle(&w, b), Ready);
    assert!(store.newest_event_of(b, "disk_lost").unwrap().is_some());
}

#[test]
fn a_close_that_cannot_wake_a_box_leaves_it_asleep_and_asks_nothing_of_the_node() {
    use moor_core::domain::box_state::BoxLifecycle::Asleep;
    use moor_core::domain::boxes::BoxState;
    let (w, hv) = live_world(32_768);
    let shut = asleep(&w, "shut", CEILING);
    let other = started(&w, "other");
    idle(&w, other);
    let (w, _) = agents_report(w, &[other], &[]);
    tick(&w, hv, with_free(8192), 0, 0);
    let store = w.cx.store().unwrap();
    let before = store.box_row(shut).unwrap().unwrap();
    let req = CloseReq {
        target: before.name.clone().into(),
        force: false,
    };
    let refused = close::run(&w.cx, &req).unwrap_err();
    let said = refused.error.to_string();
    assert!(said.contains("waiting for capacity on hv"), "{said}");
    assert!(!refused.hints.iter().any(|h| h.contains("on its own")));
    assert_eq!(lifecycle(&w, shut), Asleep);
    assert_eq!(lifecycle(&w, other), Ready, "nothing suspended for a close");
    tick(&w, hv, with_free(8192), 0, 0);
    tick(&w, hv, with_free(8192), 0, 0);
    assert_eq!(lifecycle(&w, other), Ready, "no demand on later reports");
    tick(&w, hv, 32_768, 0, 0);
    assert_eq!(lifecycle(&w, shut), Asleep, "no wake on its own");
    let after = store.box_row(shut).unwrap().unwrap();
    assert_eq!(after.session, before.session);
    assert_eq!(after.state_reason, before.state_reason);
    assert_eq!(after.boxed.map(|b| b.state), Some(BoxState::Asleep));
}

#[test]
fn a_close_with_room_still_wakes_and_closes_the_box() {
    use moor_core::domain::box_state::BoxLifecycle::Closed;
    let (w, hv) = live_world(32_768);
    let shut = asleep(&w, "shut", 1024);
    tick(&w, hv, 32_768, 0, 0);
    let name = w.cx.store().unwrap().box_row(shut).unwrap().unwrap().name;
    let req = CloseReq {
        target: name.into(),
        force: true,
    };
    close::run(&w.cx, &req).unwrap();
    assert_eq!(lifecycle(&w, shut), Closed);
}

/// What the provider reports each of `boxes` holds on the host.
fn holding(w: &World, hv: i64, boxes: &[(BoxId, u64)]) {
    let store = w.cx.store().unwrap();
    let controller = store.controller_id().unwrap();
    let rows: Vec<BoxHostMemory> = boxes
        .iter()
        .map(|(id, mib)| BoxHostMemory {
            machine_id: hv,
            controller: controller.clone(),
            box_id: *id,
            host_memory_mib: *mib,
            resume_memory_mib: None,
        })
        .collect();
    store.set_box_memory(&rows).unwrap();
}

/// A head too big for `hv`, a small Box behind it, both waiting, and one Box up holding `held`.
fn stuck_queue(held: Option<u64>) -> (World, i64, BoxId, BoxId, BoxId) {
    let (w, hv) = live_world(32_768);
    let big = asleep(&w, "big", CEILING);
    let small = asleep(&w, "small", 1024);
    let up = started(&w, "up");
    let (w, _) = agents_report(w, &[], &[up]);
    if let Some(mib) = held {
        holding(&w, hv, &[(up, mib)]);
    }
    tick(&w, hv, with_free(8192), 0, 0);
    assert!(wake_up(&w, big).is_err());
    assert!(wake_up(&w, small).is_err());
    (w, hv, big, small, up)
}

#[test]
fn a_small_box_passes_a_head_that_could_not_fit_even_with_nothing_running() {
    let (w, hv, big, small, _) = stuck_queue(Some(1024));
    tick(&w, hv, with_free(8192), 0, 0);
    assert_eq!(states(&w, &[big, small]), [Pending, Ready]);
}

#[test]
fn no_box_passes_a_head_that_stopping_the_running_boxes_would_fit() {
    let (w, hv, big, small, _) = stuck_queue(Some(16 * 1024));
    tick(&w, hv, with_free(8192), 0, 0);
    assert_eq!(states(&w, &[big, small]), [Pending, Pending]);
}

#[test]
fn no_box_passes_when_a_running_box_does_not_say_what_it_holds() {
    let (w, hv, big, small, _) = stuck_queue(None);
    tick(&w, hv, with_free(8192), 0, 0);
    assert_eq!(states(&w, &[big, small]), [Pending, Pending]);
}

#[test]
fn passing_stops_once_the_head_could_fit() {
    let (w, hv, big, small, up) = stuck_queue(Some(1024));
    holding(&w, hv, &[(up, 16 * 1024)]);
    tick(&w, hv, with_free(8192), 0, 0);
    assert_eq!(states(&w, &[big, small]), [Pending, Pending]);
}

#[test]
fn nothing_is_suspended_for_a_head_that_could_not_fit_even_so() {
    let (w, hv) = live_world(32_768);
    let big = asleep(&w, "big", CEILING);
    let other = started(&w, "other");
    idle(&w, other);
    let (w, _) = agents_report(w, &[other], &[]);
    holding(&w, hv, &[(other, 1024)]);
    tick(&w, hv, with_free(1024), 0, 0);
    assert!(wake_up(&w, big).is_err());
    for _ in 0..4 {
        tick(&w, hv, with_free(1024), 0, 0);
    }
    assert_eq!(lifecycle(&w, other), Ready);
}

#[test]
fn an_idle_box_is_still_suspended_for_a_head_that_stopping_it_would_fit() {
    let (w, hv) = live_world(32_768);
    let big = asleep(&w, "big", CEILING);
    let other = started(&w, "other");
    idle(&w, other);
    let (w, _) = agents_report(w, &[other], &[]);
    holding(&w, hv, &[(other, 20 * 1024)]);
    tick(&w, hv, with_free(1024), 0, 0);
    assert!(wake_up(&w, big).is_err());
    tick(&w, hv, with_free(1024), 0, 0);
    assert_eq!(lifecycle(&w, other), Asleep);
}

#[test]
fn asleep_since_stays_when_a_waiting_box_is_only_given_a_reason() {
    let (w, hv) = live_world(32_768);
    let big = asleep(&w, "big", CEILING);
    tick(&w, hv, with_free(1024), 0, 0);
    assert!(wake_up(&w, big).is_err());
    let at = |w: &World| {
        w.cx.store()
            .unwrap()
            .box_row(big)
            .unwrap()
            .unwrap()
            .boxed
            .unwrap()
            .updated_at
    };
    let before = at(&w);
    tick(&w, hv, with_free(1024), 0, 0);
    assert_eq!(at(&w), before);
}
