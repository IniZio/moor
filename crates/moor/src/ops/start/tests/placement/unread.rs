//! A Box whose agent cannot be read may be mid-turn, so a suspend treats it as working.

use std::sync::Arc;

use moor_core::activity::Probe;
use moor_core::domain::box_state::BoxLifecycle::{Asleep, Pending, Ready};

use super::pressure::{tick, BELOW_RESERVE};
use super::stored_check::holds;
use super::*;
use crate::testing::agent;
use crate::testing::engine::FakeEngine;

/// How long a read of an agent stands for a dropped one.
const UNREAD_AFTER: std::time::Duration = std::time::Duration::from_secs(120);

/// Moor read `id`'s agent just now.
fn seen_idle(w: &World, id: BoxId) {
    let store = w.cx.store().unwrap();
    let live = store.live_session(id).unwrap().unwrap();
    store.note_agent_read(live.id, w.cx.clock().now()).unwrap();
}

/// Reads of `id`'s agent have failed for as long as the last good read stands in.
pub(super) fn unread(w: &World, id: BoxId) {
    seen_idle(w, id);
    w.cx.clock().sleep(UNREAD_AFTER);
}

/// Reads fail for `dropped`, say idle for `read`, and say busy for `busy`.
fn reads(mut w: World, dropped: &[BoxId], read: &[BoxId], busy: &[BoxId]) -> World {
    let session = |id: &BoxId| format!("moor-{}", id.get());
    let at = w.cx.clock().now();
    let mut engine = FakeEngine::new();
    for id in dropped {
        engine = engine.probes(&session(id), Probe::TimedOut);
    }
    for id in read {
        engine = engine.probes(&session(id), agent::idle(at));
    }
    for id in busy {
        engine = engine.probes(&session(id), agent::working(at));
    }
    w.cx = w.cx.with_engine(Arc::new(engine));
    w
}

/// Box `a`, idle longest and unread, and Box `b`, idle and read.
fn unread_and_read() -> (World, i64, BoxId, BoxId) {
    let (w, hv) = live_world(32_768);
    let (a, b) = (started(&w, "a"), started(&w, "b"));
    idle(&w, a);
    idle(&w, b);
    unread(&w, a);
    seen_idle(&w, b);
    (reads(w, &[a], &[b], &[]), hv, a, b)
}

/// Box `only`, idle and unread, the one Box on `hv`.
fn only_unread() -> (World, i64, BoxId) {
    let (w, hv) = live_world(32_768);
    let only = started(&w, "only");
    idle(&w, only);
    unread(&w, only);
    (reads(w, &[only], &[], &[]), hv, only)
}

fn still_idle(w: &World, id: BoxId) -> bool {
    let row = w.cx.store().unwrap().box_row(id).unwrap().unwrap();
    row.session.is_quiet() && row.idle_since.is_some()
}

#[test]
fn a_waiting_box_takes_room_from_a_read_box_before_an_unread_one() {
    let (w, hv, a, b) = unread_and_read();
    report(&w, hv, SHORT, None);
    waits(&w, "c");
    assert_eq!(lifecycle(&w, a), Ready);
    assert_eq!(lifecycle(&w, b), Asleep);
    assert_eq!(asleep_reason(&w, b).as_deref(), Some("made room for c"));
}

#[test]
fn a_waiting_box_never_takes_room_from_an_unread_box() {
    let (w, hv, only) = only_unread();
    report(&w, hv, SHORT, None);
    waits(&w, "c");
    assert_eq!(lifecycle(&w, only), Ready);
    assert!(still_idle(&w, only));
}

#[test]
fn plain_pressure_suspends_a_read_box_before_an_unread_one() {
    let (w, hv, a, b) = unread_and_read();
    for _ in 0..3 {
        tick(&w, hv, 32_768, 1000, 0);
    }
    assert_eq!(lifecycle(&w, a), Ready);
    assert_eq!(lifecycle(&w, b), Asleep);
    assert_eq!(
        asleep_reason(&w, b).as_deref(),
        Some("memory pressure on hv")
    );
}

#[test]
fn plain_pressure_never_suspends_an_unread_box() {
    let (w, hv, only) = only_unread();
    for _ in 0..4 {
        tick(&w, hv, 32_768, 1000, 0);
    }
    assert_eq!(lifecycle(&w, only), Ready);
}

#[test]
fn critical_memory_takes_a_read_box_before_an_unread_one() {
    let (w, hv, a, b) = unread_and_read();
    tick(&w, hv, 512, 0, 0);
    assert_eq!(lifecycle(&w, a), Ready);
    assert_eq!(lifecycle(&w, b), Asleep);
}

/// Busy `x`, then idle and unread `a`, the newest Box on `hv`.
fn working_then_unread() -> (World, i64, BoxId, BoxId) {
    let (w, hv) = live_world(32_768);
    let (x, a) = (started(&w, "x"), started(&w, "a"));
    idle(&w, a);
    unread(&w, a);
    (reads(w, &[a], &[], &[x]), hv, x, a)
}

#[test]
fn an_unread_box_goes_only_as_a_last_resort() {
    let (w, hv, x, a) = working_then_unread();
    tick(&w, hv, BELOW_RESERVE, 0, 0);
    assert_eq!(lifecycle(&w, a), Pending);
    assert_eq!(lifecycle(&w, x), Ready);
    assert_eq!(
        asleep_reason(&w, a).as_deref(),
        Some("last-resort suspend: memory below the host reserve on hv")
    );
}

#[test]
fn the_only_box_is_left_running_when_it_is_unread_and_memory_is_critical() {
    let (w, hv, only) = only_unread();
    tick(&w, hv, 512, 0, 0);
    assert_eq!(lifecycle(&w, only), Ready);
}

#[test]
fn nothing_is_stored_about_what_an_unread_box_holds() {
    let (w, hv, _, a) = working_then_unread();
    holds(&w, a, "ahead=0 dirty=0");
    tick(&w, hv, BELOW_RESERVE, 0, 0);
    assert_eq!(lifecycle(&w, a), Pending);
    let check = w.cx.store().unwrap().suspend_check(a).unwrap();
    assert_eq!(check, None);
}

#[test]
fn a_box_read_again_is_first_in_line_again() {
    let (w, hv, only) = only_unread();
    tick(&w, hv, 32_768, 1000, 0);
    seen_idle(&w, only);
    tick(&w, hv, 32_768, 1000, 0);
    tick(&w, hv, 32_768, 1000, 0);
    assert_eq!(lifecycle(&w, only), Asleep);
}

#[test]
fn a_box_whose_agent_has_ended_is_no_longer_unread() {
    let (w, _, only) = only_unread();
    let store = w.cx.store().unwrap();
    let now = w.cx.clock().now();
    assert!(store.agent_unread(only, now).unwrap());
    store.end_current_session(only, "exited", Some(0)).unwrap();
    assert!(!store.agent_unread(only, now).unwrap());
}
