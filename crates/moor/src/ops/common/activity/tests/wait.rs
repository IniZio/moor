//! A wait the Box shows to be empty goes idle; a wait it cannot rule out stays.

use std::time::Duration;

use moor_core::activity::{WaitCheck, STALL_AFTER};
use moor_core::config::DEFAULT_SLEEP_AFTER;
use moor_core::domain::session_state::SessionState;
use moor_core::seam::clock::Clock;

use super::turn::steer;
use super::*;
use crate::ops::common::autosleep;
use crate::testing::clock::FakeClock;

const ON: &str = "Run ci, tests, lint for all six packages";

struct Rig {
    cx: Cx,
    id: BoxId,
    engine: Arc<FakeEngine>,
    driver: Arc<FakeDriver>,
    clock: Arc<FakeClock>,
}

fn rig(tail: &str) -> (tempfile::TempDir, Rig) {
    let dir = tempfile::tempdir().unwrap();
    let mut probes = hook(HookKind::Idle);
    probes.hook = HookRead::Fresh(HookRecord {
        message: Some("Both are running.".to_owned()),
        at: NOW,
        ..rec(HookKind::Waiting {
            on: vec![ON.to_owned()],
        })
    });
    probes.pane = PaneRead::Fresh(PaneObs {
        status: PaneStatus::Idle,
        tail: tail.to_owned(),
        prompt: None,
        exit_marker: None,
    });
    let engine = Arc::new(FakeEngine::new().probes(SESSION, probes));
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
    (
        dir,
        Rig {
            cx,
            id,
            engine,
            driver,
            clock,
        },
    )
}

impl Rig {
    fn read_after(&self, quiet: Duration) -> Reading {
        let now = NOW.saturating_add(quiet);
        let gone = now.as_millis() - self.clock.now().as_millis();
        self.clock
            .sleep(Duration::from_millis(u64::try_from(gone).unwrap()));
        let store = self.cx.store().unwrap();
        read(
            &self.cx,
            store,
            &row(&self.cx, self.id),
            now,
            PROBE_DEADLINE,
        )
        .unwrap()
    }

    fn settle_after(&self, quiet: Duration) -> Reading {
        let r = self.read_after(quiet);
        let store = self.cx.store().unwrap();
        settle(&self.cx, store, &row(&self.cx, self.id), &r).unwrap();
        r
    }

    fn sleeps_asked(&self) -> usize {
        autosleep::run(&self.cx, self.cx.store().unwrap(), self.id, false);
        let calls = self.driver.calls();
        calls.iter().filter(|c| c.starts_with("sleep ")).count()
    }

    fn session(&self) -> SessionState {
        row(&self.cx, self.id).session
    }

    fn kinds(&self) -> Vec<String> {
        let events = self.cx.store().unwrap().events(self.id, 100).unwrap();
        events.into_iter().map(|e| e.kind).collect()
    }
}

#[test]
fn a_wait_on_nothing_goes_idle_and_keeps_the_agents_last_message() {
    let (_d, rig) = rig("");
    rig.engine.set_wait_check(SESSION, WaitCheck::Nothing);
    let r = rig.settle_after(STALL_AFTER);
    assert_eq!(state(&r), (AgentState::Idle, Source::Hooks));
    assert!(!r.busy());
    assert!(r.may_quiet);
    assert!(rig.session().is_quiet());
    let kinds = rig.kinds();
    assert!(kinds.iter().any(|k| k == "idled"), "{kinds:?}");
    assert!(kinds.iter().any(|k| k == "agent_said"), "{kinds:?}");
    let cached = rig.cx.store().unwrap().hook(rig.id).unwrap().unwrap();
    assert_eq!(cached.record.kind, HookKind::Idle);
}

#[test]
fn a_box_past_its_wait_sleeps_after_the_idle_time() {
    let (_d, rig) = rig("");
    rig.engine.set_wait_check(SESSION, WaitCheck::Nothing);
    rig.settle_after(STALL_AFTER);
    rig.settle_after(STALL_AFTER + DEFAULT_SLEEP_AFTER);
    assert_eq!(rig.sleeps_asked(), 1);
}

#[test]
fn a_box_on_a_running_build_does_not_sleep_past_the_idle_time() {
    let (_d, rig) = rig("");
    rig.engine.set_wait_check(SESSION, WaitCheck::Running);
    rig.settle_after(STALL_AFTER);
    rig.settle_after(STALL_AFTER + DEFAULT_SLEEP_AFTER);
    assert_eq!(rig.sleeps_asked(), 0);
}

#[test]
fn a_running_build_keeps_the_box_working_however_quiet_it_is() {
    let (_d, rig) = rig("");
    rig.engine.set_wait_check(SESSION, WaitCheck::Running);
    for quiet in [STALL_AFTER, STALL_AFTER * 6, STALL_AFTER * 11] {
        let r = rig.settle_after(quiet);
        assert_eq!(
            state(&r).0,
            AgentState::Waiting {
                on: vec![ON.to_owned()]
            }
        );
        assert!(r.busy());
        assert!(!r.may_quiet);
        assert!(!rig.session().is_quiet());
        assert!(row(&rig.cx, rig.id).idle_since.is_none());
    }
    assert!(!rig.kinds().iter().any(|k| k == "idled"));
}

#[test]
fn a_box_that_cannot_tell_stays_working() {
    let (_d, rig) = rig("");
    rig.engine.set_wait_check(SESSION, WaitCheck::Unknown);
    let r = rig.settle_after(STALL_AFTER * 11);
    assert!(r.busy());
    assert!(!rig.session().is_quiet());
}

#[test]
fn a_wait_under_the_bound_is_not_ended_even_on_an_empty_box() {
    let (_d, rig) = rig("");
    rig.engine.set_wait_check(SESSION, WaitCheck::Nothing);
    let r = rig.settle_after(STALL_AFTER.checked_sub(Duration::from_millis(1)).unwrap());
    assert!(r.busy());
    assert!(!rig.session().is_quiet());
}

#[test]
fn a_pane_still_showing_background_work_keeps_the_box_working() {
    let (_d, rig) = rig("1 background task running");
    rig.engine.set_wait_check(SESSION, WaitCheck::Nothing);
    let r = rig.settle_after(STALL_AFTER);
    assert_eq!(state(&r), (AgentState::Working, Source::Pane));
    assert!(!rig.session().is_quiet());
}

#[test]
fn work_seen_again_after_the_wait_ended_brings_the_box_back() {
    let (_d, rig) = rig("");
    rig.engine.set_wait_check(SESSION, WaitCheck::Nothing);
    rig.settle_after(STALL_AFTER);
    assert!(rig.session().is_quiet());
    rig.engine.set_wait_check(SESSION, WaitCheck::Running);
    rig.settle_after(STALL_AFTER + Duration::from_secs(5));
    assert!(!rig.session().is_quiet());
    assert!(row(&rig.cx, rig.id).idle_since.is_none());
}

#[test]
fn a_dropped_read_never_ends_a_wait() {
    let (_d, rig) = rig("");
    rig.engine.set_wait_check(SESSION, WaitCheck::Running);
    rig.settle_after(STALL_AFTER);
    rig.engine.set_wait_check(SESSION, WaitCheck::Nothing);
    rig.engine.set_probes(
        SESSION,
        Probes {
            hook: HookRead::TimedOut,
            pane: PaneRead::TimedOut,
        },
    );
    let r = rig.settle_after(STALL_AFTER + Duration::from_secs(5));
    assert!(r.busy());
    assert!(!rig.session().is_quiet());
}

#[test]
fn a_steered_turn_ends_when_its_wait_on_nothing_ends() {
    let (_d, rig) = rig("");
    steer(&rig.cx, rig.id);
    rig.engine.set_wait_check(SESSION, WaitCheck::Running);
    rig.settle_after(STALL_AFTER);
    assert!(!rig.kinds().iter().any(|k| k == "turn_done"));
    rig.engine.set_wait_check(SESSION, WaitCheck::Nothing);
    rig.settle_after(STALL_AFTER + Duration::from_secs(5));
    assert!(rig.kinds().iter().any(|k| k == "turn_done"));
    assert!(rig.session().is_quiet());
}
