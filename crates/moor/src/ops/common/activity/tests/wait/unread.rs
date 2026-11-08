//! A read that drops out says nothing about the agent, so it moves no timer.

use std::time::Duration;

use moor_core::activity::Probe;
use moor_core::config::DEFAULT_SLEEP_AFTER;
use moor_core::seam::clock::Clock;

use super::super::*;
use crate::ops::common::autosleep;
use crate::testing::agent::{said, waiting, working};
use crate::testing::clock::FakeClock;

const SAID: &str = "done 9:55 AM";

struct Rig {
    cx: Cx,
    id: BoxId,
    engine: Arc<FakeEngine>,
    driver: Arc<FakeDriver>,
    clock: Arc<FakeClock>,
}

const READ_TTL: Duration = Duration::from_secs(120);

fn dropped() -> Probe {
    Probe::TimedOut
}

fn idle_saying() -> Probe {
    said(SAID, NOW)
}

/// A Box that went Idle at `NOW` on a fresh report.
fn rig() -> (tempfile::TempDir, Rig) {
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(FakeEngine::new().probes(SESSION, idle_saying()));
    let clock = Arc::new(FakeClock::new(NOW));
    let driver = Arc::new(FakeDriver::new());
    let cx = Cx::fake(
        dir.path(),
        driver.clone(),
        Arc::new(FakeSurface::new()),
        Arc::new(ScriptedProc::new()),
    )
    .with_engine(engine.clone())
    .with_clock(clock.clone());
    let id = working_box(&cx, true);
    let rig = Rig {
        cx,
        id,
        engine,
        driver,
        clock,
    };
    rig.pass(Duration::ZERO);
    assert!(is_idle(&row(&rig.cx, rig.id)));
    (dir, rig)
}

impl Rig {
    /// One settle pass `later` from now, as the controller runs it; its reading comes back.
    fn pass(&self, later: Duration) -> Reading {
        self.clock.sleep(later);
        let store = self.cx.store().unwrap();
        let now = self.clock.now();
        let r = read(
            &self.cx,
            store,
            &row(&self.cx, self.id),
            now,
            PROBE_DEADLINE,
        )
        .unwrap();
        settle(&self.cx, store, &row(&self.cx, self.id), &r).unwrap();
        autosleep::run(&self.cx, store, self.id, autosleep::AgentRead::of(&r));
        r
    }

    fn reads(&self, probes: Probe) {
        self.engine.set_probes(SESSION, probes);
    }

    fn count(&self, kind: &str) -> usize {
        let events = self.cx.store().unwrap().events(self.id, 1000).unwrap();
        events.iter().filter(|e| e.kind == kind).count()
    }

    fn sleeps(&self) -> usize {
        let calls = self.driver.calls();
        calls.iter().filter(|c| c.starts_with("sleep ")).count()
    }
}

#[test]
fn a_dropped_read_after_a_while_leaves_an_idle_box_idle() {
    let (_d, rig) = rig();
    let since = row(&rig.cx, rig.id).idle_since;
    rig.reads(dropped());
    let r = rig.pass(READ_TTL);
    assert_eq!(state(&r), (AgentState::Unknown, Source::Timeout));
    assert!(is_idle(&row(&rig.cx, rig.id)));
    assert_eq!(row(&rig.cx, rig.id).idle_since, since);
    assert_eq!(rig.count("busy"), 0);
}

#[test]
fn reads_that_keep_dropping_out_write_one_idle_and_let_the_box_sleep() {
    let (_d, rig) = rig();
    for gap in [163, 147] {
        rig.reads(dropped());
        rig.pass(Duration::from_secs(gap));
        assert_eq!(rig.sleeps(), 0, "an unread agent is not put to sleep");
        rig.reads(idle_saying());
        rig.pass(Duration::from_secs(10));
    }
    assert_eq!(rig.count("busy"), 0);
    assert_eq!(rig.count("idled"), 1);
    assert_eq!(rig.count("agent_said"), 1);
    assert_eq!(rig.sleeps(), 1);
}

#[test]
fn an_unread_agent_does_not_sleep_on_its_idle_time() {
    let (_d, rig) = rig();
    rig.reads(dropped());
    rig.pass(DEFAULT_SLEEP_AFTER * 2);
    assert_eq!(rig.sleeps(), 0);
    assert!(is_idle(&row(&rig.cx, rig.id)));
}

#[test]
fn an_unread_agent_still_sleeps_at_the_awake_cap() {
    let (_d, rig) = rig();
    rig.reads(dropped());
    rig.pass(Duration::from_hours(2));
    assert_eq!(rig.sleeps(), 1);
}

#[test]
fn a_working_status_after_a_drop_out_brings_the_box_back_and_stops_its_sleep() {
    let (_d, rig) = rig();
    rig.reads(dropped());
    rig.pass(DEFAULT_SLEEP_AFTER * 2);
    rig.reads(working(NOW));
    rig.pass(Duration::from_secs(5));
    assert!(is_working(&row(&rig.cx, rig.id)));
    assert_eq!(row(&rig.cx, rig.id).idle_since, None);
    assert_eq!(rig.count("busy"), 1);
    assert_eq!(rig.sleeps(), 0);
}

#[test]
fn a_wait_on_background_work_brings_the_box_back() {
    let (_d, rig) = rig();
    rig.reads(waiting(&["cargo test"], false, NOW));
    rig.pass(Duration::from_secs(5));
    assert!(is_working(&row(&rig.cx, rig.id)));
}

#[test]
fn an_unread_box_with_a_run_in_flight_does_not_sleep() {
    let (_d, rig) = rig();
    let _run = begin(&rig.cx, rig.id);
    rig.reads(dropped());
    rig.pass(DEFAULT_SLEEP_AFTER * 2);
    rig.reads(idle_saying());
    rig.pass(Duration::from_secs(5));
    assert_eq!(rig.sleeps(), 0);
}

#[test]
fn a_blocked_agent_sleeps_on_the_shorter_prompt_time() {
    let (dir, rig) = rig();
    let config =
        moor_core::config::parse_user("u.yaml", "box: {sleep_after: 1h, sleep_after_blocked: 5m}");
    let cx = Cx::fake(
        dir.path(),
        rig.driver.clone(),
        Arc::new(FakeSurface::new()),
        Arc::new(ScriptedProc::new()),
    )
    .with_engine(rig.engine.clone())
    .with_clock(rig.clock.clone())
    .with_config(config.unwrap());
    rig.clock.sleep(DEFAULT_SLEEP_AFTER);
    let store = cx.store().unwrap();
    autosleep::run(&cx, store, rig.id, autosleep::AgentRead::Plain);
    assert_eq!(rig.sleeps(), 0, "an hour has not passed");
    let blocked = Reading::new(
        Some(moor_core::activity::Activity {
            state: AgentState::NeedsInput {
                question: None,
                options: Vec::new(),
            },
            source: Source::Status,
        }),
        PaneLiveness::Live,
        true,
        false,
    );
    autosleep::run(&cx, store, rig.id, autosleep::AgentRead::of(&blocked));
    assert_eq!(rig.sleeps(), 1);
}

#[test]
fn an_agent_with_no_successful_read_does_not_count_as_unread() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(FakeEngine::new());
    let (cx, _) = cx(dir.path(), &engine);
    let id = working_box(&cx, true);
    assert!(!cx.store().unwrap().agent_unread(id, NOW).unwrap());
}

#[test]
fn an_agent_counts_as_unread_two_minutes_after_its_last_successful_read() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(FakeEngine::new().probes(SESSION, working(NOW)));
    let (cx, _) = cx(dir.path(), &engine);
    let id = working_box(&cx, true);
    run_once(&cx, id);
    let store = cx.store().unwrap();
    let before = NOW.saturating_add(READ_TTL.saturating_sub(Duration::from_millis(1)));
    assert!(!store.agent_unread(id, before).unwrap());
    assert!(store
        .agent_unread(id, NOW.saturating_add(READ_TTL))
        .unwrap());
}

#[test]
fn a_failed_read_does_not_refresh_the_last_successful_read() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(FakeEngine::new().probes(SESSION, working(NOW)));
    let (cx, _) = cx(dir.path(), &engine);
    let id = working_box(&cx, true);
    run_once(&cx, id);
    let later = NOW.saturating_add(READ_TTL);
    let cx = cx.with_engine(Arc::new(FakeEngine::new().probes(SESSION, dropped())));
    let store = cx.store().unwrap();
    read(&cx, store, &row(&cx, id), later, PROBE_DEADLINE).unwrap();
    assert!(store.agent_unread(id, later).unwrap());
}
