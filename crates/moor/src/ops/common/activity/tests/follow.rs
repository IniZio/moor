//! The quick reads after a steer, which put the agent's first line on record within a second.

use std::sync::Arc;
use std::time::Duration;

use moor_core::seam::clock::Clock;

use super::*;
use moor_core::domain::inbox::InboxKind;

use crate::cx::owners::Cmd;
use crate::ops::common::activity::{follow, Steered};
use crate::testing::agent::{at_turn, doing};
use crate::testing::clock::FakeClock;
use crate::testing::inbox::sent;

fn lines(cx: &Cx, id: BoxId) -> Vec<String> {
    let events = cx.store().unwrap().events(id, 100).unwrap();
    events
        .iter()
        .filter(|e| e.kind == "agent_doing")
        .filter_map(|e| e.payload.get("text")?.as_str().map(str::to_owned))
        .collect()
}

/// A Box whose agent was steered while idle at turn 1, its status set at `seen()`, read through
/// `engine` on `clock`.
fn steered_box(dir: &Path, engine: &Arc<FakeEngine>, clock: &Arc<FakeClock>) -> (Cx, BoxId) {
    let (cx, _) = cx(dir, engine);
    let cx = cx.with_clock(clock.clone());
    let id = working_box(&cx, true);
    sent(cx.store().unwrap(), id, Some(1), Some(seen().as_millis()));
    (cx, id)
}

fn elapsed(clock: &FakeClock) -> Duration {
    Duration::from_millis((clock.now().as_millis() - NOW.as_millis()).unsigned_abs())
}

#[test]
fn the_first_line_after_a_steer_is_on_record_within_a_second() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(
        FakeEngine::new()
            .probes(SESSION, at_turn(doing("Read: README.md", seen()), 1))
            .then_probes(SESSION, idle(seen()))
            .then_probes(SESSION, idle(seen())),
    );
    let clock = Arc::new(FakeClock::new(NOW));
    let (cx, id) = steered_box(dir.path(), &engine, &clock);

    follow(&cx, cx.store().unwrap(), id);

    assert_eq!(lines(&cx, id), ["Read: README.md"]);
    assert!(
        elapsed(&clock) <= Duration::from_secs(1),
        "on record {:?} after the steer",
        elapsed(&clock)
    );
    assert_eq!(
        engine.probe_count(SESSION),
        3,
        "the reads stop once one shows the line"
    );
}

#[test]
fn an_agent_that_never_takes_the_steer_up_is_read_for_fifteen_seconds_at_most() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(FakeEngine::new().probes(SESSION, idle(seen())));
    let clock = Arc::new(FakeClock::new(NOW));
    let (cx, id) = steered_box(dir.path(), &engine, &clock);

    follow(&cx, cx.store().unwrap(), id);

    assert!(lines(&cx, id).is_empty());
    let reads = engine.probe_count(SESSION);
    assert!(
        (55..=61).contains(&reads),
        "{reads} reads in {:?}",
        elapsed(&clock)
    );
    assert!(elapsed(&clock) < Duration::from_secs(16));
}

#[test]
fn a_failed_read_ends_the_quick_reads() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(
        FakeEngine::new()
            .probes(SESSION, idle(seen()))
            .fail_probe(SESSION),
    );
    let clock = Arc::new(FakeClock::new(NOW));
    let (cx, id) = steered_box(dir.path(), &engine, &clock);

    follow(&cx, cx.store().unwrap(), id);

    assert_eq!(engine.probe_count(SESSION), 1);
    assert!(
        cx.take_warnings()
            .iter()
            .any(|w| w.code == "agent_read_degraded"),
        "the failed read is reported as before"
    );
}

#[test]
fn a_steer_the_agent_already_answered_is_read_once() {
    let dir = tempfile::tempdir().unwrap();
    let later = Ts::from_millis(NOW.as_millis() - 500);
    let engine = Arc::new(FakeEngine::new().probes(SESSION, at_turn(idle(later), 2)));
    let clock = Arc::new(FakeClock::new(NOW));
    let (cx, id) = steered_box(dir.path(), &engine, &clock);

    follow(&cx, cx.store().unwrap(), id);

    assert_eq!(engine.probe_count(SESSION), 1);
    assert!(
        cx.store().unwrap().sent(id).unwrap().is_empty(),
        "its turn ended"
    );
}

#[test]
fn a_delivered_steer_has_the_controller_follow_its_box() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(FakeEngine::new().probes(SESSION, idle(seen())));
    let (cx, _) = cx(dir.path(), &engine);
    let steered = Arc::new(Steered::default());
    let cx = cx.with_env(|e| e.sharing(Arc::clone(&steered)));
    let id = working_box(&cx, true);
    let store = cx.store().unwrap();
    store.enqueue(id, InboxKind::Steer, "go", "test").unwrap();
    let turn = cx.turn(id, Cmd::Steer).unwrap();
    crate::ops::steer::deliver(&cx, store, id).unwrap();
    drop(turn);

    assert_eq!(steered.next(Duration::ZERO), Some(id));
    assert_eq!(steered.next(Duration::ZERO), None);
}
