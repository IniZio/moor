//! A workflow fired by hand: the claim, the refusal, and how it sits beside the cron firings.

use moor_core::request::CloseReq;
use moor_core::seam::clock::Ts;
use moor_core::view::workflow::{RunNowLine, RunNowValue};
use moor_core::workflow::{NewRun, RunState, RunTrigger, WorkflowId};

use super::{repo, rig, Rig, DAY, FIRST_DUE};
use crate::ops::workflow::now::run_with;
use crate::ops::workflow::{run_now, run_start};
use crate::testing::cx::REQUESTER_USER;
use crate::testing::runs::{delivered, refused, running, skipped};

/// Records a run of the repo as the tick or a person would, never starting a Box.
fn record_run(r: &Rig, id: WorkflowId, due: i64, (state, trigger): (RunState, RunTrigger)) {
    let run = NewRun {
        workflow: id,
        repo: repo(),
        due_at: Ts::from_millis(due),
        state,
        recorded_at: Ts::from_millis(due),
        plan: None,
        trigger,
    };
    assert!(r.cx.store().unwrap().record_workflow_run(&run).unwrap());
}

/// `run_workflow` for the repos named, as the workflow's owner, each Box started on the spot.
fn fire(r: &Rig, repos: &[&str]) -> Result<RunNowValue, String> {
    let repos: Vec<String> = repos.iter().map(ToString::to_string).collect();
    let start = |job| {
        run_start(&r.cx, &job);
        Ok(())
    };
    run_with(
        &r.cx,
        r.cx.store().unwrap(),
        REQUESTER_USER,
        ("daily-fix", &repos, None),
        &start,
    )
    .map_err(|e| e.error.to_string())
}

fn only(value: RunNowValue) -> RunNowLine {
    assert_eq!(value.runs.len(), 1, "{value:?}");
    value.runs.into_iter().next().unwrap()
}

#[test]
fn a_manual_run_starts_a_box_named_to_the_second_and_records_its_trigger() {
    let r = rig();
    r.answer_repo();
    let id = r.put(180);
    r.advance_to(5_000);
    let line = only(fire(&r, &[]).unwrap());
    assert_eq!(line.status, "started", "{line:?}");
    assert_eq!(
        line.box_name.as_deref(),
        Some("n/w1-daily-fix-m19700101000005")
    );
    let run = r.latest(id);
    assert_eq!(run.trigger, RunTrigger::Manual);
    assert_eq!(run.due_at, Ts::from_millis(5_000));
    assert!(
        matches!(run.state, RunState::Running { .. }),
        "{:?}",
        run.state
    );
    assert_eq!(r.open_boxes(), 1);
}

#[test]
fn a_manual_run_on_a_repo_with_an_open_run_is_refused_and_records_nothing() {
    let r = rig();
    r.answer_repo();
    let id = r.put(180);
    r.advance_to(FIRST_DUE + 1_000);
    r.pass();
    let before = r.runs(id);
    assert_eq!(before.len(), 1);

    let line = only(fire(&r, &[]).unwrap());
    assert_eq!(line.status, "refused", "{line:?}");
    let why = line.reason.unwrap_or_default();
    assert!(why.contains("n/w1-daily-fix-197001010900"), "{why}");
    assert_eq!(r.runs(id), before, "nothing was recorded");
    assert_eq!(r.open_boxes(), 1);
}

#[test]
fn an_open_cron_run_that_was_recorded_after_a_manual_one_still_blocks() {
    // The manual run is newer by due time, the cron one by when it was recorded.
    let r = rig();
    r.answer_repo();
    let id = r.put(180);
    record_run(
        &r,
        id,
        FIRST_DUE + 500,
        (delivered(7, FIRST_DUE + 600), RunTrigger::Manual),
    );
    record_run(&r, id, FIRST_DUE, (running(8), RunTrigger::Cron));
    r.advance_to(FIRST_DUE + 1_000);
    let line = only(fire(&r, &[]).unwrap());
    assert_eq!(line.status, "refused", "{line:?}");
    assert_eq!(r.runs(id).len(), 2);
}

#[test]
fn a_manual_run_leaves_a_missed_due_time_to_fire() {
    let r = rig();
    r.answer_repo();
    let id = r.put(180);
    record_run(
        &r,
        id,
        FIRST_DUE + 500,
        (delivered(7, FIRST_DUE + 600), RunTrigger::Manual),
    );
    r.advance_to(FIRST_DUE + 1_000);
    assert!(r.pass().is_empty());
    let cron: Vec<_> = r
        .runs(id)
        .into_iter()
        .filter(|run| run.trigger == RunTrigger::Cron)
        .collect();
    assert_eq!(
        cron.len(),
        1,
        "09:00 fires though a manual run came after it"
    );
    let fired = cron.first().unwrap();
    assert_eq!(fired.due_at, Ts::from_millis(FIRST_DUE));
    assert!(matches!(fired.state, RunState::Running { .. }));
}

#[test]
fn a_cron_run_due_while_a_manual_run_is_open_is_skipped() {
    let r = rig();
    r.answer_repo();
    let id = r.put(180);
    let line = only(fire(&r, &[]).unwrap());
    assert_eq!(line.status, "started", "{line:?}");
    r.advance_to(FIRST_DUE + 1_000);
    r.pass();
    let run = r.latest(id);
    assert_eq!(run.trigger, RunTrigger::Cron);
    assert_eq!(run.state, skipped());
    assert_eq!(r.open_boxes(), 1, "no second Box");
}

#[test]
fn a_repo_the_workflow_does_not_list_or_a_stale_revision_is_refused() {
    let r = rig();
    r.answer_repo();
    r.put(180);
    let why = fire(&r, &["o/other"]).unwrap_err();
    assert!(why.contains("o/other"), "{why}");
    let stale = run_now(
        &r.cx,
        r.cx.store().unwrap(),
        REQUESTER_USER,
        ("daily-fix", &[], Some(99)),
    )
    .unwrap_err();
    assert!(stale.error.to_string().contains("revision"), "{stale:?}");
    assert_eq!(r.open_boxes(), 0);
}

#[test]
fn a_timed_out_run_blocks_while_its_box_is_open_and_not_once_it_is_closed() {
    let r = rig();
    r.answer_repo();
    let id = r.put(30);
    r.advance_to(FIRST_DUE + 1_000);
    r.pass();
    r.advance_to(FIRST_DUE + 1_000 + 31 * 60_000);
    r.pass();
    let before = r.runs(id);
    let line = only(fire(&r, &[]).unwrap());
    assert_eq!(line.status, "refused", "{line:?}");
    assert_eq!(r.runs(id), before);

    let box_id = r.latest_box(id);
    let name = r.cx.store().unwrap().box_row(box_id).unwrap().unwrap().name;
    let close = CloseReq {
        target: name.into(),
        force: true,
    };
    crate::ops::close::run(&r.cx, &close).unwrap();
    let line = only(fire(&r, &[]).unwrap());
    assert_eq!(line.status, "started", "{line:?}");
}

#[test]
fn a_manual_firing_of_a_disabled_workflow_runs() {
    let r = rig();
    r.answer_repo();
    r.put(180);
    let store = r.cx.store().unwrap();
    let name = moor_core::workflow::WorkflowName::parse("daily-fix").unwrap();
    assert!(store
        .set_workflow_enabled(REQUESTER_USER, &name, false)
        .unwrap());
    let line = only(fire(&r, &[]).unwrap());
    assert_eq!(line.status, "started", "{line:?}");
}

#[test]
fn skips_between_failures_do_not_hide_the_breaker_streak() {
    let r = rig();
    let id = r.put(180);
    let record = |due: i64, state: RunState| record_run(&r, id, due, (state, RunTrigger::Cron));
    record(FIRST_DUE, refused("no"));
    record(FIRST_DUE + DAY, refused("no"));
    for day in 2..30 {
        record(FIRST_DUE + day * DAY, skipped());
    }
    record(FIRST_DUE + 30 * DAY, refused("no"));
    r.advance_to(FIRST_DUE + 31 * DAY);
    let lines = r.pass();
    assert!(lines.iter().any(|l| l.contains("disabled")), "{lines:?}");
}

#[test]
fn a_manual_run_answers_while_its_box_is_still_setting_up() {
    use std::sync::{mpsc, Mutex};
    use std::time::Duration;

    let r = rig();
    r.answer_repo();
    let id = r.put(180);
    let (release, held) = mpsc::channel::<()>();
    let held = Mutex::new(held);
    r.driver.on_exec(move || {
        let _ = held.lock().unwrap().recv();
    });
    let (answer, answered) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let store = r.cx.store().unwrap();
        let _ = answer.send(run_now(
            &r.cx,
            store,
            REQUESTER_USER,
            ("daily-fix", &[], None),
        ));
        r
    });
    let got = answered.recv_timeout(Duration::from_secs(5));
    drop(release);
    let r = worker.join().unwrap();
    let line = only(
        got.expect("run_workflow answered while setup was still running")
            .map_err(|e| e.error.to_string())
            .unwrap(),
    );
    assert_eq!(line.status, "started", "{line:?}");
    let running = || matches!(r.latest(id).state, RunState::Running { .. });
    assert!(
        crate::testing::driver::eventually(running),
        "{:?}",
        r.latest(id)
    );
    let store = r.cx.store().unwrap();
    let started = store.box_row(r.latest_box(id)).unwrap().unwrap().name;
    assert_eq!(line.box_name.as_deref(), Some(started.as_str()));
    assert_eq!(r.open_boxes(), 1);
}
