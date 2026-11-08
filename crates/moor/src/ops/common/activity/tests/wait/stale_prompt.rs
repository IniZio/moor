//! A prompt the agent answered stops blocking the Box, and a steer that never shows a turn ends.

use std::time::Duration;

use moor_core::activity::STALL_AFTER;
use moor_core::config::DEFAULT_SLEEP_AFTER;
use moor_core::domain::decision::ExtraEvent;
use moor_core::seam::clock::Clock;

use super::super::*;
use crate::ops::common::autosleep;
use crate::ops::steer::tell;
use crate::testing::agent::{idle, on_screen, permission, working};
use crate::testing::clock::FakeClock;

const ASKED: &str = "Claude needs your permission";
const GRACE: Duration = Duration::from_secs(30);

struct Rig {
    _dir: tempfile::TempDir,
    cx: Cx,
    id: BoxId,
    engine: Arc<FakeEngine>,
    driver: Arc<FakeDriver>,
    clock: Arc<FakeClock>,
}

fn rig(probe: Probe) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(FakeEngine::new().probes(SESSION, probe));
    let clock = Arc::new(FakeClock::new(NOW));
    let driver = Arc::new(FakeDriver::new());
    let cx = Cx::fake_with_clock(
        dir.path(),
        driver.clone(),
        Arc::new(FakeSurface::new()),
        Arc::new(ScriptedProc::new()),
        moor_core::config::UserConfig::default(),
        clock.clone(),
    )
    .with_engine(engine.clone());
    let id = working_box(&cx, true);
    Rig {
        _dir: dir,
        cx,
        id,
        engine,
        driver,
        clock,
    }
}

impl Rig {
    fn read_later(&self, later: Duration) -> Reading {
        self.clock.sleep(later);
        let store = self.cx.store().unwrap();
        let now = self.clock.now();
        read(
            &self.cx,
            store,
            &row(&self.cx, self.id),
            now,
            PROBE_DEADLINE,
        )
        .unwrap()
    }

    fn settle_later(&self, later: Duration) -> Reading {
        let r = self.read_later(later);
        settle(
            &self.cx,
            self.cx.store().unwrap(),
            &row(&self.cx, self.id),
            &r,
        )
        .unwrap();
        r
    }

    fn sleeps_after(&self, reading: &Reading) -> usize {
        let store = self.cx.store().unwrap();
        autosleep::run(&self.cx, store, self.id, autosleep::AgentRead::of(reading));
        let calls = self.driver.calls();
        calls.iter().filter(|c| c.starts_with("sleep ")).count()
    }

    fn count(&self, kind: &str) -> usize {
        let events = self.cx.store().unwrap().events(self.id, 100).unwrap();
        events.iter().filter(|e| e.kind == kind).count()
    }

    fn quiet(&self) -> bool {
        row(&self.cx, self.id).session.is_quiet()
    }

    fn steer_text(&self) -> Result<(), crate::error::CommandError> {
        let row = row(&self.cx, self.id);
        tell(&self.cx, self.cx.store().unwrap(), &row, "go on".to_owned())
    }

    /// A typed steer sent now, whose turn has to end past `turn_before` ended turns.
    fn steered(&self, turn_before: Option<u64>) {
        let event = ExtraEvent::Steered {
            session: SESSION.to_owned(),
            message: "go".to_owned(),
            submit_retried: false,
            turn_before,
        };
        let store = self.cx.store().unwrap();
        store.apply(Decision::new(self.id).event(event)).unwrap();
    }
}

#[test]
fn a_prompt_the_agent_moved_past_reads_as_idle_and_lets_a_steer_through() {
    let rig = rig(permission(ASKED, NOW));
    let r = rig.settle_later(GRACE);
    assert!(r.blocked());
    rig.engine.set_probes(SESSION, idle(rig.clock.now()));
    let r = rig.settle_later(GRACE);
    assert_eq!(state(&r), (AgentState::Idle, Source::Status));
    assert!(!r.blocked());
    assert!(rig.quiet());
    rig.steer_text().unwrap();
    assert_eq!(rig.engine.steers().len(), 1);
}

#[test]
fn a_box_whose_prompt_was_answered_sleeps_after_the_idle_time() {
    let rig = rig(permission(ASKED, NOW));
    rig.settle_later(GRACE);
    rig.engine.set_probes(SESSION, idle(rig.clock.now()));
    rig.settle_later(GRACE);
    let r = rig.settle_later(DEFAULT_SLEEP_AFTER);
    assert_eq!(rig.sleeps_after(&r), 1);
}

#[test]
fn a_prompt_still_blocked_blocks_a_steer_however_old_it_is() {
    let rig = rig(on_screen(permission(ASKED, NOW), "❯ 1. Yes\n  2. No\n"));
    let r = rig.settle_later(STALL_AFTER * 6);
    assert!(r.blocked());
    assert!(matches!(state(&r).0, AgentState::NeedsInput { .. }));
    assert!(rig.steer_text().is_err());
    assert!(rig.engine.steers().is_empty());
}

/// A Box whose agent last reported `probe`, with a steer sent after it that no turn answers.
fn steered_after(probe: Probe, turn_before: u64) -> Rig {
    let rig = rig(probe);
    rig.settle_later(Duration::ZERO);
    rig.steered(Some(turn_before));
    rig.cx
        .store()
        .unwrap()
        .apply(Decision::new(rig.id).session(SessionInput::AgentBusy))
        .ok();
    rig
}

#[test]
fn a_steer_with_no_turn_to_show_ends_after_the_quiet_bound() {
    let rig = steered_after(idle(NOW), 1);
    rig.settle_later(STALL_AFTER.checked_sub(Duration::from_secs(1)).unwrap());
    assert_eq!(rig.count("turn_done"), 0);
    assert!(!rig.quiet());
    rig.settle_later(Duration::from_secs(1));
    assert_eq!(rig.count("turn_done"), 1);
    assert!(rig.quiet());
}

#[test]
fn a_steer_on_a_turn_still_working_never_ends_on_the_bound() {
    let rig = steered_after(working(NOW), 0);
    rig.settle_later(STALL_AFTER * 6);
    assert_eq!(rig.count("turn_done"), 0);
    assert!(!rig.quiet());
}

#[test]
fn a_steer_does_not_end_on_the_bound_while_the_agent_is_unread() {
    let rig = steered_after(idle(NOW), 1);
    rig.engine.set_probes(SESSION, Probe::TimedOut);
    rig.settle_later(STALL_AFTER * 2);
    assert_eq!(rig.count("turn_done"), 0);
    assert!(!rig.quiet());
}

#[test]
fn a_steer_sent_recently_is_not_ended_by_an_old_status() {
    let rig = steered_after(idle(NOW), 1);
    rig.clock.sleep(STALL_AFTER * 2);
    rig.steered(Some(1));
    rig.settle_later(Duration::from_secs(5));
    assert_eq!(rig.count("turn_done"), 0);
    assert!(!rig.quiet());
}

#[test]
fn an_earlier_turn_just_ending_does_not_end_an_old_steer_behind_it() {
    let rig = steered_after(working(NOW), 1);
    rig.clock.sleep(STALL_AFTER);
    rig.engine.set_probes(SESSION, idle(rig.clock.now()));
    rig.settle_later(Duration::from_secs(5));
    assert_eq!(rig.count("turn_done"), 0);
    assert!(!rig.quiet());
}
