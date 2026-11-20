//! Tests for a run fired by hand beside the cron ones.

use super::{
    breaker, daily, facts, finished, refused, run, timed_out, ts, verdicts, DAY, DAY0, HOUR,
};
use crate::decide::workflow::{workflow_due, PriorRun, WorkflowFacts, WorkflowVerdict};
use crate::seam::clock::Ts;
use crate::workflow::{box_name, RunCause, RunState, RunTrigger, WorkflowId, WorkflowRun};

/// `run` fired by hand.
fn manual(run: WorkflowRun) -> WorkflowRun {
    WorkflowRun {
        cause: RunCause::Manual { pull: None },
        ..run
    }
}

#[test]
fn a_manual_run_leaves_workflow_due_firing_a_missed_due_time() {
    // A manual run finished at 03:00 after yesterday's cron run. The facts the tick reads leave
    // it out of `last_due`, so today's 09:00 still fires, and its overlap fact is the manual run.
    let w = daily();
    let yesterday = DAY0 - DAY + 9 * HOUR;
    let now = DAY0 + 9 * HOUR + 1_000;
    let f = facts(Some(yesterday), false);
    assert_eq!(
        verdicts(&w, &f, now),
        WorkflowVerdict::Start {
            due_at: ts(DAY0 + 9 * HOUR)
        }
    );
}

#[test]
fn a_cron_run_due_while_a_manual_run_is_open_records_skipped_overlap() {
    let w = daily();
    let f = WorkflowFacts {
        last_due: None,
        prior: Some(PriorRun::Started),
    };
    let now = DAY0 + 9 * HOUR + 1_000;
    assert_eq!(
        workflow_due(&w, &f, ts(now)),
        WorkflowVerdict::SkipOverlap {
            due_at: ts(DAY0 + 9 * HOUR)
        }
    );
}

#[test]
fn three_failed_manual_runs_leave_the_breaker_untripped() {
    let base = DAY0 - 10 * DAY;
    let runs = [
        manual(refused("a/b", base)),
        manual(timed_out("a/b", base + DAY)),
        manual(refused("a/b", base + 2 * DAY)),
        manual(timed_out("a/b", base + 3 * DAY)),
    ];
    assert!(!breaker(&runs));
}

#[test]
fn a_manual_failure_does_not_add_to_the_cron_streak() {
    let base = DAY0 - 10 * DAY;
    let runs = [
        refused("a/b", base),
        refused("a/b", base + DAY),
        manual(refused("a/b", base + 2 * DAY)),
    ];
    assert!(!breaker(&runs), "two cron failures and a manual one");
    let more = [runs.to_vec(), vec![refused("a/b", base + 3 * DAY)]].concat();
    assert!(breaker(&more), "the third cron failure still trips it");
}

#[test]
fn a_manual_delivery_resets_the_streak() {
    let base = DAY0 - 10 * DAY;
    let runs = [
        refused("a/b", base),
        refused("a/b", base + DAY),
        manual(finished("a/b", base + 2 * DAY)),
        refused("a/b", base + 3 * DAY),
        timed_out("a/b", base + 4 * DAY),
    ];
    assert!(!breaker(&runs));
    let more = [runs.to_vec(), vec![refused("a/b", base + 5 * DAY)]].concat();
    assert!(breaker(&more));
}

#[test]
fn a_manual_run_in_the_same_minute_as_a_cron_run_gets_another_box_name() {
    // 2026-11-18 05:12 UTC, and the second the manual run was fired at.
    let id = WorkflowId::new(3);
    let due = Ts::from_millis(1_794_978_720_000);
    let at = Ts::from_millis(1_794_978_750_000);
    let cron = box_name(id, "deps", due, &RunCause::Cron).unwrap();
    let hand = box_name(id, "deps", at, &RunCause::Manual { pull: None }).unwrap();
    assert_eq!(cron.as_str(), "deps/w3-202611180512");
    assert_eq!(hand.as_str(), "deps/w3-m20261118051230");
    let same_minute = box_name(id, "deps", due, &RunCause::Manual { pull: None }).unwrap();
    assert_ne!(cron, same_minute);
}

#[test]
fn a_run_is_fired_by_cron_unless_made_manual() {
    let r = run("a/b", DAY0, DAY0, RunState::Queued);
    assert_eq!(r.cause.trigger(), RunTrigger::Cron);
    assert_eq!(manual(r).cause.trigger(), RunTrigger::Manual);
}
