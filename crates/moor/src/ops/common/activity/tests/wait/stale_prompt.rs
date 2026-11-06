//! A permission prompt that ended without a hook report no longer blocks the Box.

use std::time::Duration;

use moor_core::activity::STALL_AFTER;
use moor_core::config::DEFAULT_SLEEP_AFTER;
use moor_core::domain::decision::ExtraEvent;
use moor_core::seam::clock::Clock;

use super::super::*;
use crate::ops::common::autosleep;
use crate::ops::steer::tell;
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

fn pane(tail: &str) -> PaneRead {
    PaneRead::Fresh(PaneObs {
        status: PaneStatus::Unknown,
        tail: tail.to_owned(),
        prompt: None,
        exit_marker: None,
    })
}

fn asked() -> HookRecord {
    HookRecord {
        message: Some(ASKED.to_owned()),
        at: NOW,
        ..rec(HookKind::Blocked)
    }
}

fn fresh(record: HookRecord, pane: PaneRead) -> Probes {
    Probes {
        hook: HookRead::Fresh(record),
        pane,
    }
}

fn rig(probes: Probes) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(FakeEngine::new().probes(SESSION, probes));
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

    /// A typed steer sent now, whose turn has to show after the report written at `before`.
    fn steered(&self, before: Ts) {
        let event = ExtraEvent::Steered {
            session: SESSION.to_owned(),
            message: "go".to_owned(),
            submit_retried: false,
            hook_before: Some(before.as_millis()),
            new_turn: true,
        };
        let store = self.cx.store().unwrap();
        store.apply(Decision::new(self.id).event(event)).unwrap();
    }
}

fn gone() -> Rig {
    let rig = rig(fresh(asked(), pane("recap\n> ")));
    rig.engine.set_prompt_gone(true);
    rig
}

#[test]
fn a_prompt_gone_from_the_pane_reads_as_idle_without_its_question() {
    let rig = gone();
    let r = rig.settle_later(GRACE);
    assert_eq!(state(&r), (AgentState::Idle, Source::Hooks));
    assert!(!r.blocked());
    assert!(rig.quiet());
    assert_eq!(rig.count("agent_said"), 0);
    let cached = rig.cx.store().unwrap().hook(rig.id).unwrap().unwrap();
    assert_eq!(cached.record.kind, HookKind::Idle);
}

#[test]
fn a_prompt_gone_from_the_pane_lets_a_steer_through_and_the_box_sleep() {
    let rig = gone();
    rig.settle_later(GRACE);
    rig.steer_text().unwrap();
    assert_eq!(rig.engine.steers().len(), 1);
    let rig = gone();
    rig.settle_later(GRACE);
    let r = rig.settle_later(DEFAULT_SLEEP_AFTER);
    assert_eq!(rig.sleeps_after(&r), 1);
}

#[test]
fn a_prompt_still_on_the_pane_blocks_a_steer_however_old_it_is() {
    let rig = rig(fresh(asked(), pane("❯ 1. Yes\n  2. No\n")));
    let r = rig.settle_later(STALL_AFTER * 6);
    assert!(r.blocked());
    assert!(matches!(state(&r).0, AgentState::NeedsInput { .. }));
    assert!(rig.steer_text().is_err());
    assert!(rig.engine.steers().is_empty());
}

#[test]
fn a_prompt_under_the_grace_stands_even_with_nothing_on_the_pane() {
    let rig = gone();
    let r = rig.settle_later(GRACE.checked_sub(Duration::from_millis(1)).unwrap());
    assert!(r.blocked());
}

#[test]
fn a_prompt_stands_while_the_pane_cannot_be_read() {
    for unseen in [PaneRead::TimedOut, PaneRead::Absent] {
        let rig = rig(fresh(asked(), unseen));
        rig.engine.set_prompt_gone(true);
        let r = rig.settle_later(STALL_AFTER);
        assert!(r.blocked());
    }
}

#[test]
fn a_cached_prompt_stands_when_the_hook_read_drops() {
    let rig = rig(fresh(asked(), pane("❯ 1. Yes\n  2. No\n")));
    rig.settle_later(Duration::ZERO);
    rig.engine.set_prompt_gone(true);
    rig.engine.set_probes(
        SESSION,
        Probes {
            hook: HookRead::TimedOut,
            pane: pane("recap\n> "),
        },
    );
    let r = rig.settle_later(GRACE);
    assert!(r.blocked());
}

/// A Box whose agent last reported `record`, with a steer sent after it that no report answers.
fn steered_after(record: HookRecord) -> Rig {
    let at = record.at;
    let rig = rig(fresh(record, PaneRead::Absent));
    rig.settle_later(Duration::ZERO);
    rig.steered(at);
    rig.cx
        .store()
        .unwrap()
        .apply(Decision::new(rig.id).session(SessionInput::AgentBusy))
        .ok();
    rig
}

fn report(kind: HookKind) -> HookRecord {
    HookRecord {
        at: NOW,
        ..rec(kind)
    }
}

#[test]
fn a_steer_with_no_turn_to_show_ends_after_the_quiet_bound() {
    let rig = steered_after(report(HookKind::Idle));
    rig.settle_later(STALL_AFTER.checked_sub(Duration::from_secs(1)).unwrap());
    assert_eq!(rig.count("turn_done"), 0);
    assert!(!rig.quiet());
    rig.settle_later(Duration::from_secs(1));
    assert_eq!(rig.count("turn_done"), 1);
    assert!(rig.quiet());
}

#[test]
fn a_steer_on_a_turn_still_busy_never_ends_on_the_bound() {
    let rig = steered_after(report(HookKind::Busy));
    rig.settle_later(STALL_AFTER * 6);
    assert_eq!(rig.count("turn_done"), 0);
    assert!(!rig.quiet());
}

#[test]
fn a_steer_does_not_end_on_the_bound_while_the_agent_is_unread() {
    let rig = steered_after(report(HookKind::Idle));
    rig.engine.set_probes(
        SESSION,
        Probes {
            hook: HookRead::TimedOut,
            pane: PaneRead::TimedOut,
        },
    );
    rig.settle_later(STALL_AFTER * 2);
    assert_eq!(rig.count("turn_done"), 0);
    assert!(!rig.quiet());
}

#[test]
fn a_steer_sent_recently_is_not_ended_by_an_old_report() {
    let rig = steered_after(report(HookKind::Idle));
    rig.clock.sleep(STALL_AFTER * 2);
    rig.steered(NOW);
    rig.settle_later(Duration::from_secs(5));
    assert_eq!(rig.count("turn_done"), 0);
    assert!(!rig.quiet());
}

#[test]
fn an_earlier_turn_just_ending_does_not_end_an_old_steer_behind_it() {
    let rig = steered_after(report(HookKind::Busy));
    rig.clock.sleep(STALL_AFTER);
    let just_now = HookRecord {
        at: rig.clock.now(),
        ..rec(HookKind::Idle)
    };
    rig.engine
        .set_probes(SESSION, fresh(just_now, PaneRead::Absent));
    rig.settle_later(Duration::from_secs(5));
    assert_eq!(rig.count("turn_done"), 0);
    assert!(!rig.quiet());
}
