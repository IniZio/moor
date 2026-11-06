//! What a Box holds is recorded as it is suspended, so a close can answer for a Box that does not
//! fit to wake.

use moor_core::domain::box_state::BoxLifecycle::{Asleep, Closed, Pending, Ready};
use moor_core::domain::boxes::BoxState;
use moor_core::domain::decision::{Decision, Field, Fields};
use moor_core::{BoxPath, ErrorKind};

use super::pressure::{agents_report, tick, BELOW_RESERVE};
use super::*;
use crate::store::SuspendCheck;

const CEILING: u64 = 23 * 1024;
const CLEAN: &str = "ahead=0 dirty=0";
const OLD_CLEAN: SuspendCheck = SuspendCheck {
    ahead: 0,
    dirty: 0,
    at: 1,
};

/// Makes the check of `id` answer `text`.
fn holds(w: &World, id: BoxId, text: &str) {
    let row = w.cx.store().unwrap().box_row(id).unwrap().unwrap();
    let b = row.boxed.unwrap();
    let path = BoxPath::under(b.workdir.as_deref().unwrap(), ".moor/boxwork").unwrap();
    w.driver
        .set_peek_text(b.handle.unwrap().as_str(), &path, text);
}

fn stored(w: &World, id: BoxId) -> Option<(u32, u32)> {
    let check = w.cx.store().unwrap().suspend_check(id).unwrap();
    check.map(|c| (c.ahead, c.dirty))
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

/// Two working Boxes on `hv`; `b`, the newest, is suspended holding `text` and too big to wake.
fn suspended_holding(text: &str) -> (World, i64, BoxId, BoxId) {
    let (w, hv) = live_world(32_768);
    let (a, b) = (started(&w, "a"), started(&w, "b"));
    guest_memory(&w, b, CEILING);
    let (w, _) = agents_report(w, &[], &[a, b]);
    holds(&w, b, text);
    tick(&w, hv, BELOW_RESERVE, 0, 0);
    assert_eq!(lifecycle(&w, b), Pending);
    (w, hv, a, b)
}

fn close_box(w: &World, id: BoxId, force: bool) -> Result<(), crate::error::CommandError> {
    let name = w.cx.store().unwrap().box_row(id).unwrap().unwrap().name;
    let req = CloseReq {
        target: name.into(),
        force,
    };
    close::run(&w.cx, &req).map(drop)
}

fn calls(w: &World, of: &str) -> usize {
    w.driver
        .calls()
        .iter()
        .filter(|c| c.starts_with(of))
        .count()
}

fn state(w: &World, id: BoxId) -> Option<BoxState> {
    let row = w.cx.store().unwrap().box_row(id).unwrap().unwrap();
    row.boxed.map(|b| b.state)
}

#[test]
fn a_suspend_records_what_the_box_holds_just_before_it() {
    let (w, _, _, b) = suspended_holding("ahead=2 dirty=1");
    assert_eq!(stored(&w, b), Some((2, 1)));
}

#[test]
fn an_idle_suspend_records_it_too() {
    let (w, hv) = live_world(32_768);
    let a = started(&w, "a");
    idle(&w, a);
    let (w, _) = agents_report(w, &[a], &[]);
    holds(&w, a, CLEAN);
    tick(&w, hv, BELOW_RESERVE, 0, 0);
    assert_eq!(lifecycle(&w, a), Asleep);
    assert_eq!(stored(&w, a), Some((0, 0)));
}

#[test]
fn a_check_that_cannot_answer_drops_any_older_answer_and_does_not_stop_the_suspend() {
    let (w, hv) = live_world(32_768);
    let (a, b) = (started(&w, "a"), started(&w, "b"));
    let (w, _) = agents_report(w, &[], &[a, b]);
    let store = w.cx.store().unwrap();
    store.set_suspend_check(b, Some(OLD_CLEAN)).unwrap();
    tick(&w, hv, BELOW_RESERVE, 0, 0);
    assert_eq!(lifecycle(&w, b), Pending);
    assert_eq!(stored(&w, b), None);
}

#[test]
fn critical_memory_suspends_without_asking_what_the_box_holds() {
    let (w, hv) = live_world(32_768);
    let (a, b) = (started(&w, "a"), started(&w, "b"));
    let (w, _) = agents_report(w, &[], &[a, b]);
    holds(&w, b, CLEAN);
    w.cx.store()
        .unwrap()
        .set_suspend_check(b, Some(OLD_CLEAN))
        .unwrap();
    let before = calls(&w, "batch");
    tick(&w, hv, 512, 0, 0);
    assert_eq!(lifecycle(&w, b), Pending);
    assert_eq!(stored(&w, b), None);
    assert_eq!(calls(&w, "batch"), before);
}

#[test]
fn a_wake_clears_the_answer_since_the_guest_can_change_again() {
    let (w, hv) = live_world(32_768);
    let (a, b) = (started(&w, "a"), started(&w, "b"));
    let (w, _) = agents_report(w, &[], &[a, b]);
    holds(&w, b, CLEAN);
    tick(&w, hv, BELOW_RESERVE, 0, 0);
    assert_eq!(stored(&w, b), Some((0, 0)));
    tick(&w, hv, 32_768, 0, 0);
    assert_eq!(lifecycle(&w, b), Ready);
    assert_eq!(stored(&w, b), None);
}

#[test]
fn a_box_found_stopped_keeps_no_answer_from_before_it_last_ran() {
    let (w, _hv) = live_world(32_768);
    let a = started(&w, "a");
    let store = w.cx.store().unwrap();
    store.set_suspend_check(a, Some(OLD_CLEAN)).unwrap();
    let row = store.box_row(a).unwrap().unwrap();
    w.driver.stop_behind(&row.boxed.unwrap().handle.unwrap());
    crate::ops::common::reconcile::run(&w.cx);
    assert_eq!(state(&w, a), Some(BoxState::Asleep));
    assert_eq!(stored(&w, a), None);
}

#[test]
fn close_of_a_clean_box_with_room_wakes_it_so_its_caches_are_saved() {
    let (w, hv) = live_world(32_768);
    let a = asleep_clean(&w);
    tick(&w, hv, 32_768, 0, 0);
    let before = calls(&w, "wake");
    close_box(&w, a, false).unwrap();
    assert_eq!(lifecycle(&w, a), Closed);
    assert_eq!(calls(&w, "wake"), before + 1);
    let store = w.cx.store().unwrap();
    assert!(store
        .newest_event_of(a, "caches_not_saved")
        .unwrap()
        .is_none());
}

/// A small Box asleep with a clean answer stored, whose live check answers clean too.
fn asleep_clean(w: &World) -> BoxId {
    let a = started(w, "small");
    guest_memory(w, a, 1024);
    holds(w, a, CLEAN);
    put_to_sleep(w, a);
    w.cx.store()
        .unwrap()
        .set_suspend_check(a, Some(OLD_CLEAN))
        .unwrap();
    a
}

#[test]
fn close_of_a_clean_box_with_no_room_deletes_it_unwoken_and_says_caches_were_lost() {
    let (w, hv, a, b) = suspended_holding(CLEAN);
    tick(&w, hv, BELOW_RESERVE, 0, 0);
    let before = calls(&w, "wake");
    close_box(&w, b, false).unwrap();
    assert_eq!(lifecycle(&w, b), Closed);
    assert_eq!(calls(&w, "wake"), before);
    assert_eq!(lifecycle(&w, a), Ready, "nothing suspended for a close");
    let store = w.cx.store().unwrap();
    assert!(store
        .newest_event_of(b, "caches_not_saved")
        .unwrap()
        .is_some());
    assert_eq!(state(&w, b), Some(BoxState::Deleted));
}

#[test]
fn close_of_a_box_holding_unsaved_work_refuses_naming_it_without_a_wake() {
    let (w, _, _, b) = suspended_holding("ahead=2 dirty=1");
    let before = calls(&w, "wake");
    let refused = close_box(&w, b, false).unwrap_err();
    assert_eq!(refused.kind, ErrorKind::Refused);
    let said = refused.error.to_string();
    assert!(
        said.contains("2 unpushed") || said.contains("2 commit"),
        "{said}"
    );
    assert_eq!(calls(&w, "wake"), before);
    assert_eq!(state(&w, b), Some(BoxState::Asleep));
}

#[test]
fn a_forced_close_ignores_the_answer() {
    let (w, _, _, b) = suspended_holding("ahead=2 dirty=1");
    close_box(&w, b, true).unwrap();
    assert_eq!(lifecycle(&w, b), Closed);
}
