//! A run's Box closes a day after the run ends, unless someone steered or opened its terminal.
#![allow(clippy::panic, reason = "a test names what it found when it fails")]
#![allow(clippy::indexing_slicing, reason = "test assertions")]

use std::time::Duration;

use moor_core::config::AutoClose;
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::domain::event::{AccessEvent, SteerSent};
use moor_core::workflow::{RunEnd, RunState};
use moor_core::BoxId;

use super::previous::{finished_first_day, lifecycle, put_to_sleep, rig_probing};
use super::Rig;
use crate::ops::{auto_close_pass, AutoCloseLooked};

const HOUR: i64 = 3_600_000;
const DAY: i64 = 24 * HOUR;

const fn cfg() -> AutoClose {
    AutoClose {
        every: Duration::from_secs(60),
        run_after: Some(Duration::from_hours(24)),
        idle_after: Some(Duration::from_hours(24 * 7)),
    }
}

fn sweep(r: &Rig) -> Vec<String> {
    let looked = AutoCloseLooked::default();
    let store = r.cx.store().unwrap();
    auto_close_pass(&r.cx, store, &|| false, (&cfg(), &looked)).unwrap()
}

/// A run that ended with its Box asleep, answering its work check with `probe`; when the run
/// ended, in unix milliseconds.
fn ended(probe: &str) -> (Rig, BoxId, i64) {
    let r = rig_probing(probe);
    r.answer_repo();
    let id = r.put(180);
    let first = finished_first_day(&r, id);
    put_to_sleep(&r, first);
    let RunState::Done(RunEnd::Ended { ended_at, .. }) = r.latest(id).state else {
        panic!("the run did not end");
    };
    (r, first, ended_at.as_millis())
}

fn events(r: &Rig, id: BoxId, kind: &str) -> Vec<serde_json::Value> {
    let all = r.cx.store().unwrap().events(id, 1000).unwrap();
    all.into_iter()
        .filter(|e| e.kind == kind)
        .map(|e| e.payload)
        .collect()
}

fn steered(r: &Rig, id: BoxId) {
    let sent = SteerSent {
        session: "s".to_owned(),
        message: "more".to_owned(),
        submit_retried: false,
        turn_before: None,
        status_before: None,
        inbox: None,
        origin: moor_core::domain::event::Origin::Person,
    };
    let d = Decision::new(id).event(ExtraEvent::Steered(sent));
    r.cx.store().unwrap().apply(d).unwrap();
}

#[test]
fn a_run_box_closes_a_day_after_the_run_ends_without_waking() {
    let (r, id, end) = ended("ahead=0 dirty=0\n");
    r.advance_to(end + DAY - 60_000);
    assert!(sweep(&r).is_empty());
    assert_eq!(lifecycle(&r, id), BoxLifecycle::Asleep);
    r.advance_to(end + DAY + 1_000);
    assert_eq!(sweep(&r), ["closed daily-fix/w1-197001010900: run_ended"]);
    assert_eq!(lifecycle(&r, id), BoxLifecycle::Closed);
    assert_eq!(events(&r, id, "closed")[0]["reason"], "run_ended");
    assert!(events(&r, id, "woke").is_empty());
}

#[test]
fn a_steer_inside_the_window_restarts_the_clock_from_the_steer() {
    let (r, id, end) = ended("ahead=0 dirty=0\n");
    r.advance_to(end + 20 * HOUR);
    steered(&r, id);
    r.advance_to(end + DAY + 1_000);
    assert!(sweep(&r).is_empty(), "a day has not passed since the steer");
    assert_eq!(lifecycle(&r, id), BoxLifecycle::Asleep);
    r.advance_to(end + 20 * HOUR + DAY + 1_000);
    sweep(&r);
    assert_eq!(lifecycle(&r, id), BoxLifecycle::Closed);
}

#[test]
fn a_terminal_opened_inside_the_window_restarts_the_clock_too() {
    let (r, id, end) = ended("ahead=0 dirty=0\n");
    r.advance_to(end + 23 * HOUR);
    let opened = ExtraEvent::Access(AccessEvent::TerminalOpened { shell: true });
    r.cx.store()
        .unwrap()
        .apply(Decision::new(id).event(opened))
        .unwrap();
    r.advance_to(end + DAY + 2 * HOUR);
    assert!(sweep(&r).is_empty());
    r.advance_to(end + 23 * HOUR + DAY + 1_000);
    sweep(&r);
    assert_eq!(lifecycle(&r, id), BoxLifecycle::Closed);
}

#[test]
fn a_run_box_holding_unpushed_work_is_marked_not_closed() {
    let (r, id, end) = ended("ahead=1 dirty=4\n");
    r.advance_to(end + DAY + 1_000);
    assert!(sweep(&r).is_empty());
    assert_eq!(lifecycle(&r, id), BoxLifecycle::Asleep);
    let held = events(&r, id, "auto_close_held");
    assert_eq!(held.len(), 1);
    assert_eq!(held[0]["due"], "run_ended");
    assert_eq!(
        held[0]["why"],
        "it holds 1 unpushed commit(s) and 4 uncommitted file(s)"
    );
    assert!(events(&r, id, "woke").is_empty());
}

#[test]
fn a_run_that_has_not_ended_keeps_its_box() {
    let r = rig_probing("ahead=0 dirty=0\n");
    r.answer_repo();
    let id = r.put(180);
    r.advance_to(super::FIRST_DUE + 1_000);
    r.pass();
    let first = r.latest_box(id);
    r.advance_to(super::FIRST_DUE + 3 * DAY);
    assert!(sweep(&r).is_empty());
    assert_ne!(lifecycle(&r, first), BoxLifecycle::Closed);
}
