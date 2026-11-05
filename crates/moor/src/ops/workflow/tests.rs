//! The workflow tick over the fake driver and a fake clock.

use std::cell::RefCell;
use std::sync::Arc;
use std::time::Duration;

use moor_core::config::UserConfig;
use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::decision::Decision;
use moor_core::domain::session_state::SessionInput;
use moor_core::name::ForgeRepo;
use moor_core::seam::clock::Ts;
use moor_core::workflow::{
    TimeoutMinutes, WorkflowDef, WorkflowId, WorkflowName, WorkflowOutcome, WorkflowRun,
    WorkflowSchedule, WorkflowState,
};
use moor_core::{BoxId, BoxPath};

use super::{resolve_interrupted, run_start, tick};
use crate::cx::Cx;
use crate::testing::cx::REQUESTER_USER;
use crate::testing::driver::FakeDriver;
use crate::testing::forge::{repo_at, FakeForge};
use crate::testing::proc::ScriptedProc;
use crate::testing::surface::FakeSurface;

const DAY: i64 = 86_400_000;
/// The first 09:00 UTC after the fake clock's start.
const FIRST_DUE: i64 = 9 * 3_600_000;

struct Rig {
    cx: Cx,
    forge: Arc<FakeForge>,
    _dir: tempfile::TempDir,
}

fn rig() -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let environ = BoxPath::restore("/proc/self/environ").unwrap();
    let driver = Arc::new((1..=6).fold(FakeDriver::new(), |d, n| {
        d.peek_text(&format!("box-{n}"), &environ, "HOME=/home/box\0")
    }));
    let forge = Arc::new(FakeForge::new());
    let cx = Cx::fake_with(
        dir.path(),
        driver,
        Arc::new(FakeSurface::new()),
        Arc::new(ScriptedProc::new()),
        UserConfig::default(),
    )
    .with_forge(forge.clone());
    Rig {
        cx,
        forge,
        _dir: dir,
    }
}

fn repo() -> ForgeRepo {
    ForgeRepo::new("o", "n")
}

impl Rig {
    /// Makes the forge answer the repo, so a start of it can go ahead.
    fn answer_repo(&self) {
        self.forge.answer(repo(), Ok(repo_at("abc123", None)));
    }

    /// Stores a daily 09:00 UTC workflow of the repo, owned by the test requester.
    fn put(&self, timeout: u32) -> WorkflowId {
        let def = WorkflowDef::new(
            WorkflowName::parse("daily-fix").unwrap(),
            WorkflowSchedule::parse("0 9 * * *", "UTC").unwrap(),
            vec![repo()],
            "fix what is broken",
            (true, TimeoutMinutes::new(timeout).unwrap()),
        )
        .unwrap();
        self.cx
            .store()
            .unwrap()
            .put_workflow(REQUESTER_USER, &def)
            .unwrap()
    }

    fn advance_to(&self, ms: i64) {
        let now = self.cx.clock().now().as_millis();
        self.cx
            .clock()
            .sleep(Duration::from_millis(u64::try_from(ms - now).unwrap()));
    }

    /// One tick that starts each claimed Box on the spot; returns what it logged.
    fn pass(&self) -> Vec<String> {
        let started = RefCell::new(Vec::new());
        let mut out = tick(
            &self.cx,
            &|job| started.borrow_mut().extend(run_start(&self.cx, &job)),
            &|| false,
        );
        out.extend(started.into_inner());
        out
    }

    /// The runs of the repo, newest first.
    fn runs(&self, id: WorkflowId) -> Vec<WorkflowRun> {
        self.cx
            .store()
            .unwrap()
            .recent_workflow_runs(id, &repo(), 50)
            .unwrap()
    }

    fn open_boxes(&self) -> usize {
        self.cx.store().unwrap().open_boxes().unwrap().len()
    }

    /// The newest run of the repo.
    fn latest(&self, id: WorkflowId) -> WorkflowRun {
        self.runs(id).into_iter().next().unwrap()
    }

    /// The Box the newest run started.
    fn latest_box(&self, id: WorkflowId) -> BoxId {
        self.latest(id).outcome.box_id().unwrap()
    }
}

#[test]
fn a_due_workflow_starts_one_box_as_its_owner_and_not_again() {
    let r = rig();
    r.answer_repo();
    let id = r.put(180);
    r.pass();
    assert_eq!(r.open_boxes(), 0, "nothing is due before 09:00");

    r.advance_to(FIRST_DUE + 1_000);
    assert!(r.pass().is_empty());
    assert_eq!(r.runs(id).len(), 1);
    assert_eq!(r.latest(id).due_at, Ts::from_millis(FIRST_DUE));
    let store = r.cx.store().unwrap();
    let row = store.box_row(r.latest_box(id)).unwrap().unwrap();
    assert_eq!(row.user, Some(REQUESTER_USER));
    assert_eq!(row.prompt.as_deref(), Some("fix what is broken"));
    assert_eq!(row.name.as_str(), "n/w1-daily-fix-197001010900");

    r.advance_to(FIRST_DUE + 60_000);
    r.pass();
    assert_eq!(r.open_boxes(), 1);
    assert_eq!(r.runs(id).len(), 1);
}

#[test]
fn a_claim_a_crash_left_is_settled_and_never_started_again() {
    let r = rig();
    r.answer_repo();
    let id = r.put(180);
    r.advance_to(FIRST_DUE + 1_000);
    let claimed = RefCell::new(0);
    tick(&r.cx, &|_job| *claimed.borrow_mut() += 1, &|| false);
    assert_eq!(*claimed.borrow(), 1);
    assert_eq!(r.latest(id).outcome, WorkflowOutcome::Starting);

    r.pass();
    assert_eq!(r.open_boxes(), 0, "the next controller does not start it");
    assert_eq!(r.runs(id).len(), 1);

    assert!(resolve_interrupted(&r.cx).is_empty());
    assert!(matches!(
        r.latest(id).outcome,
        WorkflowOutcome::Refused { .. }
    ));
    assert_eq!(r.open_boxes(), 0);
    assert!(r
        .cx
        .store()
        .unwrap()
        .starting_workflow_runs()
        .unwrap()
        .is_empty());
}

#[test]
fn a_claim_whose_box_was_made_becomes_that_run() {
    let r = rig();
    r.answer_repo();
    let id = r.put(180);
    r.advance_to(FIRST_DUE + 1_000);
    let job = RefCell::new(None);
    tick(&r.cx, &|j| *job.borrow_mut() = Some(j), &|| false);
    let job = job.into_inner().unwrap();
    let started = crate::ops::start::run(
        &r.cx,
        &moor_core::StartReq {
            name: job.name.clone(),
            prompt: Some(job.prompt.clone()),
            issue: None,
            base: None,
            model: None,
            node: None,
            no_agent: false,
            resume: false,
            repo: Some(job.repo),
            also: None,
            parent: None,
            identity: None,
        },
    )
    .unwrap();
    resolve_interrupted(&r.cx);
    assert_eq!(
        r.latest(id).outcome,
        WorkflowOutcome::Started {
            box_id: BoxId::new(started.boxed.id)
        }
    );
}

#[test]
fn a_claim_whose_box_failed_its_setup_before_the_restart_is_refused() {
    let r = rig();
    r.answer_repo();
    let id = r.put(180);
    r.advance_to(FIRST_DUE + 1_000);
    let job = RefCell::new(None);
    tick(&r.cx, &|j| *job.borrow_mut() = Some(j), &|| false);
    let job = job.into_inner().unwrap();
    let started = crate::ops::start::run(
        &r.cx,
        &moor_core::StartReq {
            name: job.name.clone(),
            prompt: Some(job.prompt.clone()),
            issue: None,
            base: None,
            model: None,
            node: None,
            no_agent: false,
            resume: false,
            repo: Some(job.repo),
            also: None,
            parent: None,
            identity: None,
        },
    )
    .unwrap();
    let failed = BoxLifecycleInput::WorkingSetupFailed {
        cause: "clone failed".to_owned(),
    };
    let box_id = BoxId::new(started.boxed.id);
    let store = r.cx.store().unwrap();
    store
        .apply(Decision::new(box_id).lifecycle(failed))
        .unwrap();
    resolve_interrupted(&r.cx);
    let outcome = r.latest(id).outcome;
    assert!(
        matches!(&outcome, WorkflowOutcome::Refused { detail } if detail.contains("clone failed")),
        "{outcome:?}"
    );
}

#[test]
fn a_firing_while_the_previous_box_is_open_is_skipped_and_recorded() {
    let r = rig();
    r.answer_repo();
    let id = r.put(180);
    r.advance_to(FIRST_DUE + 1_000);
    r.pass();
    r.advance_to(FIRST_DUE + DAY + 1_000);
    r.pass();
    assert_eq!(r.latest(id).outcome, WorkflowOutcome::SkippedOverlap);
    assert_eq!(r.latest(id).due_at, Ts::from_millis(FIRST_DUE + DAY));
    assert_eq!(r.open_boxes(), 1);
}

#[test]
fn a_refused_start_is_recorded_with_its_message_and_makes_no_box() {
    let r = rig();
    let id = r.put(180);
    r.advance_to(FIRST_DUE + 1_000);
    r.pass();
    let outcome = r.latest(id).outcome;
    assert!(
        matches!(&outcome, WorkflowOutcome::Refused { detail } if !detail.is_empty()),
        "{outcome:?}"
    );
    assert_eq!(r.open_boxes(), 0);
}

#[test]
fn a_run_past_its_timeout_ends_its_session_and_keeps_its_box() {
    let r = rig();
    r.answer_repo();
    let id = r.put(30);
    r.advance_to(FIRST_DUE + 1_000);
    r.pass();
    let box_id = r.latest_box(id);
    let store = r.cx.store().unwrap();
    assert!(store.live_session(box_id).unwrap().is_some());

    r.advance_to(FIRST_DUE + 1_000 + 29 * 60_000);
    r.pass();
    assert!(matches!(
        r.latest(id).outcome,
        WorkflowOutcome::Started { .. }
    ));

    r.advance_to(FIRST_DUE + 1_000 + 31 * 60_000);
    r.pass();
    let ended_at = Ts::from_millis(FIRST_DUE + 1_000 + 31 * 60_000);
    assert_eq!(
        r.latest(id).outcome,
        WorkflowOutcome::TimedOut { box_id, ended_at }
    );
    assert!(store.live_session(box_id).unwrap().is_none());
    assert_eq!(r.open_boxes(), 1, "the Box stays for inspection");

    r.advance_to(FIRST_DUE + DAY + 1_000);
    r.pass();
    assert_eq!(r.latest(id).outcome, WorkflowOutcome::SkippedOverlap);
}

#[test]
fn a_run_finishes_once_its_agent_has_been_quiet_a_while() {
    let r = rig();
    r.answer_repo();
    let id = r.put(180);
    r.advance_to(FIRST_DUE + 1_000);
    r.pass();
    let box_id = r.latest_box(id);
    let store = r.cx.store().unwrap();
    store
        .apply(Decision::new(box_id).session(SessionInput::AgentQuiet))
        .unwrap();

    r.pass();
    assert!(
        matches!(r.latest(id).outcome, WorkflowOutcome::Started { .. }),
        "a moment of quiet"
    );
    r.advance_to(FIRST_DUE + 1_000 + 3 * 60_000);
    r.pass();
    assert!(matches!(
        r.latest(id).outcome,
        WorkflowOutcome::Finished { .. }
    ));
}

#[test]
fn a_closed_box_ends_its_run() {
    let r = rig();
    r.answer_repo();
    let id = r.put(180);
    r.advance_to(FIRST_DUE + 1_000);
    r.pass();
    let box_id = r.latest_box(id);
    let closed = moor_core::domain::box_state::BoxLifecycleInput::Close { reason: None };
    r.cx.store()
        .unwrap()
        .apply(
            Decision::new(box_id)
                .lifecycle(closed)
                .session(SessionInput::End),
        )
        .unwrap();
    r.pass();
    assert!(matches!(
        r.latest(id).outcome,
        WorkflowOutcome::Finished { .. }
    ));
}

#[test]
fn three_failed_runs_in_a_row_disable_the_workflow_with_a_reason() {
    let r = rig();
    let id = r.put(180);
    for day in 0..3 {
        r.advance_to(FIRST_DUE + day * DAY + 1_000);
        r.pass();
    }
    assert_eq!(r.runs(id).len(), 3);
    let lines = r.pass();
    assert!(lines.iter().any(|l| l.contains("disabled")), "{lines:?}");
    let w =
        r.cx.store()
            .unwrap()
            .workflow(REQUESTER_USER, &WorkflowName::parse("daily-fix").unwrap());
    let reason = "o/n: the last 3 runs were refused or timed out".to_owned();
    assert_eq!(
        w.unwrap().unwrap().state,
        WorkflowState::Disabled {
            reason: Some(reason)
        }
    );

    r.advance_to(FIRST_DUE + 3 * DAY + 1_000);
    r.pass();
    assert_eq!(r.runs(id).len(), 3, "a disabled workflow fires nothing");
}

#[test]
fn a_skip_does_not_unblock_the_next_firing_while_the_run_is_unfinished() {
    let r = rig();
    r.answer_repo();
    let id = r.put(180);
    r.advance_to(FIRST_DUE + 1_000);
    r.pass();
    for day in 1..=3 {
        r.advance_to(FIRST_DUE + day * DAY + 1_000);
        r.pass();
        assert_eq!(
            r.latest(id).outcome,
            WorkflowOutcome::SkippedOverlap,
            "day {day}"
        );
    }
    assert_eq!(r.open_boxes(), 1);
}

#[test]
fn a_finished_run_with_an_open_box_gets_a_new_box_at_the_next_due_time() {
    let r = rig();
    r.answer_repo();
    let id = r.put(180);
    r.advance_to(FIRST_DUE + 1_000);
    r.pass();
    let first = r.latest_box(id);
    let store = r.cx.store().unwrap();
    store
        .apply(Decision::new(first).session(SessionInput::AgentQuiet))
        .unwrap();
    r.advance_to(FIRST_DUE + 1_000 + 3 * 60_000);
    r.pass();
    assert!(matches!(
        r.latest(id).outcome,
        WorkflowOutcome::Finished { .. }
    ));

    r.advance_to(FIRST_DUE + DAY + 1_000);
    r.pass();
    let second = r.latest_box(id);
    assert_ne!(second, first);
    assert_eq!(
        r.open_boxes(),
        2,
        "the finished Box stays open beside the new one"
    );
    let name = store.box_row(second).unwrap().unwrap().name;
    assert_eq!(name.as_str(), "n/w1-daily-fix-197001020900");
}

#[test]
fn a_timed_out_run_with_an_open_box_blocks_however_many_skips_follow() {
    let r = rig();
    r.answer_repo();
    let id = r.put(30);
    r.advance_to(FIRST_DUE + 1_000);
    r.pass();
    r.advance_to(FIRST_DUE + 1_000 + 31 * 60_000);
    r.pass();
    assert!(matches!(
        r.latest(id).outcome,
        WorkflowOutcome::TimedOut { .. }
    ));
    for day in 1..=30 {
        r.advance_to(FIRST_DUE + day * DAY + 1_000);
        r.pass();
        assert_eq!(r.open_boxes(), 1, "day {day}");
    }
    assert_eq!(r.latest(id).outcome, WorkflowOutcome::SkippedOverlap);
}

#[test]
fn skips_between_failures_do_not_hide_the_breaker_streak() {
    let r = rig();
    let id = r.put(180);
    let store = r.cx.store().unwrap();
    let record = |due: i64, outcome: WorkflowOutcome| {
        let run = WorkflowRun {
            workflow: id,
            repo: repo(),
            due_at: Ts::from_millis(due),
            outcome,
            recorded_at: Ts::from_millis(due),
        };
        assert!(store.record_workflow_run(&run).unwrap());
    };
    let refused = || WorkflowOutcome::Refused {
        detail: "no".to_owned(),
    };
    record(FIRST_DUE, refused());
    record(FIRST_DUE + DAY, refused());
    for day in 2..30 {
        record(FIRST_DUE + day * DAY, WorkflowOutcome::SkippedOverlap);
    }
    record(FIRST_DUE + 30 * DAY, refused());
    r.advance_to(FIRST_DUE + 31 * DAY);
    let lines = r.pass();
    assert!(lines.iter().any(|l| l.contains("disabled")), "{lines:?}");
}

#[test]
fn a_box_whose_setup_fails_refuses_its_run_and_three_of_them_disable_the_workflow() {
    let r = rig();
    r.answer_repo();
    let id = r.put(180);
    for day in 0..3_i64 {
        r.advance_to(FIRST_DUE + day * DAY + 1_000);
        r.pass();
        let box_id = r.latest_box(id);
        let store = r.cx.store().unwrap();
        let name = store.box_row(box_id).unwrap().unwrap().name;
        let failed = BoxLifecycleInput::WorkingSetupFailed {
            cause: "setup line `npm ci` failed".to_owned(),
        };
        store
            .apply(Decision::new(box_id).lifecycle(failed))
            .unwrap();
        r.advance_to(FIRST_DUE + day * DAY + 41_000);
        let lines = r.pass();
        assert_eq!(
            lines.iter().any(|l| l.contains("disabled")),
            day == 2,
            "{lines:?}"
        );
        let outcome = r.latest(id).outcome;
        let named = |detail: &String| detail.contains(name.as_str()) && detail.contains("npm ci");
        assert!(
            matches!(&outcome, WorkflowOutcome::Refused { detail } if named(detail)),
            "day {day}: {outcome:?}"
        );
        assert_eq!(
            i64::try_from(r.open_boxes()).unwrap(),
            day + 1,
            "Boxes are kept"
        );
    }
    assert_eq!(r.runs(id).len(), 3, "each due time started its own Box");
}
