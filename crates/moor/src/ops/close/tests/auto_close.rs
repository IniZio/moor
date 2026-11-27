//! The auto-close pass over a one-off Box: its pull request, its idle days, and work that holds it.
#![allow(clippy::panic, reason = "a test names what it found when it fails")]
#![allow(clippy::indexing_slicing, reason = "test assertions")]

use std::sync::Arc;
use std::time::Duration;

use moor_core::config::AutoClose;
use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::domain::event::AccessEvent;
use moor_core::domain::hands::{HandsInc, HandsInput};
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::forge::{BranchPull, Pull, PullState};
use moor_core::BoxPath;

use super::{admit_on, proc, req, row, run, Fx};
use crate::cx::Cx;
use crate::ops::{auto_close_pass, AutoCloseLooked};
use crate::store::SuspendCheck;
use crate::testing::cx::box_repo;
use crate::testing::driver::FakeDriver;
use crate::testing::forge::FakeForge;

const BRANCH: &str = "moor/1-t";
const DAY: u64 = 86_400;

/// A Ready, quiet one-off Box on `BRANCH` whose probe reads `boxwork`.
struct OneOff {
    f: Fx,
    forge: Arc<FakeForge>,
    looked: AutoCloseLooked,
}

fn one_off(boxwork: &str) -> OneOff {
    one_off_on(FakeDriver::new(), boxwork)
}

/// A quiet Box that the pass would close once it has been idle long enough.
fn one_off_on(driver: FakeDriver, boxwork: &str) -> OneOff {
    let dir = tempfile::tempdir().unwrap();
    let path = BoxPath::under("/work/box-1", ".moor/boxwork").unwrap();
    let driver = Arc::new(driver.peek_text("box-1", &path, boxwork));
    let forge = Arc::new(FakeForge::new());
    let proc = proc();
    let cx = Cx::fake(dir.path(), driver.clone(), proc.clone()).with_forge(forge.clone());
    let id = admit_on(&cx, &driver, None, ("t", Some(BRANCH)));
    cx.store()
        .unwrap()
        .apply(Decision::new(id).session(SessionInput::AgentQuiet))
        .unwrap();
    OneOff {
        f: Fx {
            dir,
            cx,
            driver,
            proc,
            id,
        },
        forge,
        looked: AutoCloseLooked::default(),
    }
}

const fn cfg() -> AutoClose {
    AutoClose {
        every: Duration::from_secs(60),
        run_after: Some(Duration::from_secs(DAY)),
        idle_after: Some(Duration::from_secs(7 * DAY)),
    }
}

impl OneOff {
    fn sweep(&self) -> Vec<String> {
        let store = self.f.cx.store().unwrap();
        auto_close_pass(&self.f.cx, store, &|| false, (&cfg(), &self.looked)).unwrap()
    }

    fn pass_secs(&self, secs: u64) -> Vec<String> {
        self.f.cx.clock().sleep(Duration::from_secs(secs));
        self.sweep()
    }

    fn pull(&self, state: Option<PullState>) {
        let pull = state.map(|state| Pull {
            number: 3,
            state,
            checks: None,
        });
        let answer = Ok(BranchPull { pushed: true, pull });
        self.forge.answer_pull(box_repo().repo, BRANCH, answer);
    }

    fn closed(&self) -> bool {
        row(&self.f).lifecycle == BoxLifecycle::Closed
    }

    fn events(&self, kind: &str) -> Vec<serde_json::Value> {
        let all = self.f.cx.store().unwrap().events(self.f.id, 1000).unwrap();
        all.into_iter()
            .filter(|e| e.kind == kind)
            .map(|e| e.payload)
            .collect()
    }

    fn slept_with(&self, check: Option<(u32, u32)>) {
        let sleep = Decision::new(self.f.id)
            .lifecycle(BoxLifecycleInput::Sleep {
                auto: false,
                reason: None,
            })
            .hands_input(HandsInc::FIRST, HandsInput::Sleep);
        self.f.cx.store().unwrap().apply(sleep).unwrap();
        let at = (self.f.id, HandsInc::FIRST);
        let check = check.map(|(ahead, dirty)| SuspendCheck {
            ahead,
            dirty,
            at: 0,
        });
        self.f
            .cx
            .store()
            .unwrap()
            .set_suspend_check(at, check)
            .unwrap();
    }

    fn slept_unchecked(&self, why: &str) {
        self.slept_with(None);
        let at = (self.f.id, HandsInc::FIRST);
        let store = self.f.cx.store().unwrap();
        store.set_suspend_unchecked(at, why).unwrap();
    }

    fn woke(&self) -> bool {
        self.f.driver.calls().iter().any(|c| c.starts_with("wake"))
    }
}

#[test]
fn a_merged_pull_request_closes_the_box_and_keeps_its_evidence() {
    let t = one_off("ahead=0 dirty=0");
    let receipt = AccessEvent::TurnChanges {
        turn: 1,
        repos: Vec::new(),
    };
    let store = t.f.cx.store().unwrap();
    store
        .apply(Decision::new(t.f.id).event(ExtraEvent::Access(receipt)))
        .unwrap();
    t.pull(Some(PullState::Merged));
    let lines = t.sweep();
    assert!(t.closed(), "{lines:?}");
    assert_eq!(lines, ["closed t: pr_merged"]);
    assert_eq!(t.events("closed")[0]["reason"], "pr_merged");
    assert_eq!(t.events("turn_changes").len(), 1, "the stored diff stays");
}

#[test]
fn a_pull_request_closed_without_a_merge_closes_the_box() {
    let t = one_off("ahead=0 dirty=0");
    t.pull(Some(PullState::Closed));
    t.sweep();
    assert!(t.closed());
    assert_eq!(t.events("closed")[0]["reason"], "pr_closed");
}

#[test]
fn an_open_or_missing_pull_request_keeps_the_box() {
    let t = one_off("ahead=0 dirty=0");
    t.pull(Some(PullState::Approved));
    assert!(t.sweep().is_empty());
    t.pull(None);
    assert!(t.pass_secs(600).is_empty());
    assert!(!t.closed());
}

#[test]
fn a_one_off_box_closes_after_seven_idle_days_and_not_before() {
    let t = one_off("ahead=0 dirty=0");
    assert!(t.pass_secs(7 * DAY - 120).is_empty());
    assert!(!t.closed());
    t.pass_secs(240);
    assert!(t.closed());
    assert_eq!(t.events("closed")[0]["reason"], "idle");
}

#[test]
fn a_steer_or_terminal_restarts_the_idle_clock() {
    let t = one_off("ahead=0 dirty=0");
    t.f.cx.clock().sleep(Duration::from_secs(6 * DAY));
    let opened = AccessEvent::TerminalOpened { shell: false };
    let store = t.f.cx.store().unwrap();
    store
        .apply(Decision::new(t.f.id).event(ExtraEvent::Access(opened)))
        .unwrap();
    assert!(t.pass_secs(2 * DAY).is_empty());
    assert!(!t.closed());
    t.pass_secs(5 * DAY + 120);
    assert!(t.closed());
}

#[test]
fn unpushed_work_holds_the_close_and_says_why_once() {
    let t = one_off("ahead=2 dirty=1");
    t.pull(Some(PullState::Merged));
    assert!(t.sweep().is_empty());
    assert!(!t.closed());
    let held = t.events("auto_close_held");
    assert_eq!(held.len(), 1);
    assert_eq!(held[0]["due"], "pr_merged");
    assert_eq!(
        held[0]["why"],
        "it holds 2 unpushed commit(s) and 1 uncommitted file(s)"
    );
    t.pass_secs(3_600);
    assert_eq!(t.events("auto_close_held").len(), 1, "told once");
    assert!(!t.closed());
}

#[test]
fn a_box_whose_agent_is_working_is_left_for_a_later_pass() {
    let t = one_off("ahead=0 dirty=0");
    let store = t.f.cx.store().unwrap();
    store
        .apply(Decision::new(t.f.id).session(SessionInput::AgentBusy))
        .unwrap();
    t.pull(Some(PullState::Merged));
    assert!(t.sweep().is_empty());
    assert!(!t.closed());
    assert!(t.events("auto_close_held").is_empty());
}

#[test]
fn an_asleep_box_closes_from_the_check_recorded_at_sleep_without_waking() {
    let t = one_off("ahead=9 dirty=9");
    t.slept_with(Some((0, 0)));
    t.pull(Some(PullState::Merged));
    let lines = t.sweep();
    assert_eq!(lines, ["closed t: pr_merged"]);
    assert!(t.closed());
    assert!(!t.woke());
}

#[test]
fn an_asleep_box_whose_record_shows_unpushed_work_stays_and_is_marked() {
    let t = one_off("ahead=0 dirty=0");
    t.slept_with(Some((1, 0)));
    t.pull(Some(PullState::Closed));
    assert!(t.sweep().is_empty());
    assert!(!t.closed());
    assert!(!t.woke());
    assert_eq!(
        t.events("auto_close_held")[0]["why"],
        "it holds 1 unpushed commit(s) and 0 uncommitted file(s)"
    );
}

#[test]
fn an_asleep_box_with_no_record_is_held_not_woken() {
    let t = one_off("ahead=0 dirty=0");
    t.slept_with(None);
    t.pull(Some(PullState::Merged));
    assert!(t.sweep().is_empty());
    assert!(!t.closed());
    assert!(!t.woke());
    let held = t.events("auto_close_held");
    assert_eq!(held.len(), 1);
    assert!(held[0]["why"].as_str().unwrap().contains("went to sleep"));
}

#[test]
fn a_manual_close_of_an_asleep_box_uses_the_record_and_never_wakes_it() {
    let t = one_off("ahead=9 dirty=9");
    t.slept_with(Some((0, 0)));
    let rng = crate::testing::rng::FakeRng::default();
    t.f.cx.store().unwrap().reserve(t.f.id, &rng).unwrap();
    let v = run(&t.f.cx, &req(false)).unwrap();
    assert_eq!(v.boxed.lifecycle, BoxLifecycle::Closed);
    assert!(!t.woke());
}

#[test]
fn a_manual_close_of_an_asleep_box_whose_record_shows_work_is_refused_unwoken() {
    let t = one_off("ahead=0 dirty=0");
    t.slept_with(Some((0, 3)));
    let e = run(&t.f.cx, &req(false)).unwrap_err();
    assert_eq!(e.kind, moor_core::ErrorKind::Refused);
    assert!(!t.woke());
    assert_eq!(row(&t.f).lifecycle, BoxLifecycle::Asleep);
}

#[test]
fn closing_an_asleep_box_unwoken_says_its_caches_were_lost_unless_the_driver_keeps_them() {
    let lost = one_off("ahead=0 dirty=0");
    lost.slept_with(Some((0, 0)));
    run(&lost.f.cx, &req(false)).unwrap();
    assert_eq!(lost.events("caches_not_saved").len(), 1);

    let kept = one_off_on(FakeDriver::new().cache_volumes(), "ahead=0 dirty=0");
    kept.slept_with(Some((0, 0)));
    run(&kept.f.cx, &req(false)).unwrap();
    assert!(kept.closed());
    assert!(!kept.woke());
    assert!(kept.events("caches_not_saved").is_empty());
}

#[test]
fn an_asleep_box_whose_sleep_kept_no_answer_is_held_with_the_reason_and_the_way_on() {
    let t = one_off("ahead=0 dirty=0");
    t.slept_unchecked("memory on its node was critical");
    t.pull(Some(PullState::Merged));
    assert!(t.sweep().is_empty());
    assert!(!t.closed());
    assert!(!t.woke());
    assert_eq!(
        t.events("auto_close_held")[0]["why"],
        "its work was not checked when it went to sleep (memory on its node was critical); \
         waking or closing it checks it"
    );
}

#[test]
fn an_asleep_box_whose_sleep_recorded_nothing_says_so() {
    let t = one_off("ahead=0 dirty=0");
    t.slept_with(None);
    t.pull(Some(PullState::Merged));
    t.sweep();
    assert_eq!(
        t.events("auto_close_held")[0]["why"],
        "its work was not checked when it went to sleep (no check was recorded); \
         waking or closing it checks it"
    );
}

#[test]
fn a_held_box_that_wakes_and_sleeps_clean_closes_on_a_later_pass() {
    let t = one_off("ahead=0 dirty=0");
    t.slept_unchecked("memory on its node was critical");
    t.pull(Some(PullState::Merged));
    assert!(t.sweep().is_empty());
    let store = t.f.cx.store().unwrap();
    store
        .apply(
            Decision::new(t.f.id)
                .lifecycle(BoxLifecycleInput::Wake)
                .hands_input(HandsInc::FIRST, HandsInput::Wake),
        )
        .unwrap();
    assert_eq!(
        store.suspend_unchecked(t.f.id).unwrap(),
        None,
        "a wake clears it"
    );
    t.slept_with(Some((0, 0)));
    let lines = t.pass_secs(3_600);
    assert_eq!(lines, ["closed t: pr_merged"]);
    assert!(!t.woke());
}
