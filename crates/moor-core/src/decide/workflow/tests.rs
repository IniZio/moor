//! Tests for the workflow tick decider.

use super::{
    workflow_breaker, workflow_due, workflow_expired, PriorRun, WorkflowExpiry, WorkflowRepoFacts,
    WorkflowVerdict,
};
use crate::name::BoxId;
use crate::name::ForgeRepo;
use crate::seam::clock::Ts;
use crate::user::UserId;
use crate::workflow::{
    TimeoutMinutes, Workflow, WorkflowId, WorkflowName, WorkflowOutcome, WorkflowRun,
    WorkflowSchedule, WorkflowState,
};

const HOUR: i64 = 3_600_000;
const DAY: i64 = 24 * HOUR;
/// 2026-07-01 00:00 UTC.
const DAY0: i64 = 1_782_864_000_000;

fn repo(s: &str) -> ForgeRepo {
    ForgeRepo::parse(s).unwrap()
}

fn ts(ms: i64) -> Ts {
    Ts::from_millis(ms)
}

fn workflow(cron: &str, zone: &str, repos: &[&str], state: WorkflowState) -> Workflow {
    Workflow {
        id: WorkflowId::new(1),
        owner: UserId::new(1),
        name: WorkflowName::parse("fix").unwrap(),
        schedule: WorkflowSchedule::parse(cron, zone).unwrap(),
        repos: repos.iter().map(|r| repo(r)).collect(),
        prompt: "go".to_owned(),
        state,
        timeout: TimeoutMinutes::DEFAULT,
        origin: None,
        created_at: ts(0),
        updated_at: ts(0),
    }
}

fn enabled_since(ms: i64) -> WorkflowState {
    WorkflowState::Enabled { since: ts(ms) }
}

fn facts(r: &str, last_due: Option<i64>, unfinished: bool) -> WorkflowRepoFacts {
    WorkflowRepoFacts {
        repo: repo(r),
        last_due: last_due.map(ts),
        prior: unfinished.then_some(PriorRun::Started),
    }
}

fn verdicts(w: &Workflow, f: &[WorkflowRepoFacts], now: i64) -> Vec<WorkflowVerdict> {
    workflow_due(w, f, ts(now))
        .into_iter()
        .map(|v| v.verdict)
        .collect()
}

fn start(due_at: i64) -> WorkflowVerdict {
    WorkflowVerdict::Start { due_at: ts(due_at) }
}

fn daily() -> Workflow {
    workflow("0 9 * * *", "UTC", &["a/b"], enabled_since(DAY0 - 30 * DAY))
}

#[test]
fn first_firing_starts_once_the_time_is_reached() {
    let w = daily();
    let f = [facts("a/b", None, false)];
    assert_eq!(
        verdicts(&w, &f, DAY0 + 9 * HOUR - 1),
        vec![start(DAY0 - 15 * HOUR)]
    );
    assert_eq!(
        verdicts(&w, &f, DAY0 + 9 * HOUR),
        vec![start(DAY0 + 9 * HOUR)]
    );
    assert_eq!(
        verdicts(&w, &f, DAY0 + 15 * HOUR),
        vec![start(DAY0 + 9 * HOUR)]
    );
}

#[test]
fn a_fired_time_does_not_fire_again() {
    let w = daily();
    let f = [facts("a/b", Some(DAY0 + 9 * HOUR), false)];
    assert_eq!(
        verdicts(&w, &f, DAY0 + 9 * HOUR),
        vec![WorkflowVerdict::NotDue]
    );
    assert_eq!(
        verdicts(&w, &f, DAY0 + DAY + 9 * HOUR - 1),
        vec![WorkflowVerdict::NotDue]
    );
    assert_eq!(
        verdicts(&w, &f, DAY0 + DAY + 9 * HOUR),
        vec![start(DAY0 + DAY + 9 * HOUR)]
    );
}

#[test]
fn downtime_fires_only_the_latest_missed_time() {
    let w = daily();
    let f = [facts("a/b", Some(DAY0 - 5 * DAY + 9 * HOUR), false)];
    assert_eq!(
        verdicts(&w, &f, DAY0 + 12 * HOUR),
        vec![start(DAY0 + 9 * HOUR)]
    );
    let after = [facts("a/b", Some(DAY0 + 9 * HOUR), false)];
    assert_eq!(
        verdicts(&w, &after, DAY0 + 12 * HOUR),
        vec![WorkflowVerdict::NotDue]
    );
}

#[test]
fn an_open_previous_box_skips_the_firing() {
    let w = daily();
    let f = [facts("a/b", Some(DAY0 - DAY + 9 * HOUR), true)];
    assert_eq!(
        verdicts(&w, &f, DAY0 + 10 * HOUR),
        vec![WorkflowVerdict::SkipOverlap {
            due_at: ts(DAY0 + 9 * HOUR)
        }]
    );
    let nothing_due = [facts("a/b", Some(DAY0 + 9 * HOUR), true)];
    assert_eq!(
        verdicts(&w, &nothing_due, DAY0 + 10 * HOUR),
        vec![WorkflowVerdict::NotDue]
    );
}

#[test]
fn a_new_workflow_does_not_fire_for_times_before_it_was_created() {
    let w = workflow(
        "0 9 * * *",
        "UTC",
        &["a/b"],
        enabled_since(DAY0 + 10 * HOUR),
    );
    let f = [facts("a/b", None, false)];
    assert_eq!(
        verdicts(&w, &f, DAY0 + 12 * HOUR),
        vec![WorkflowVerdict::NotDue]
    );
    assert_eq!(
        verdicts(&w, &f, DAY0 + DAY + 9 * HOUR),
        vec![start(DAY0 + DAY + 9 * HOUR)]
    );
}

#[test]
fn a_time_exactly_at_creation_fires() {
    let w = workflow("0 9 * * *", "UTC", &["a/b"], enabled_since(DAY0 + 9 * HOUR));
    let f = [facts("a/b", None, false)];
    assert_eq!(
        verdicts(&w, &f, DAY0 + 9 * HOUR + 30_000),
        vec![start(DAY0 + 9 * HOUR)]
    );
}

#[test]
fn a_re_enabled_workflow_does_not_fire_for_times_while_it_was_off() {
    let w = workflow(
        "0 9 * * *",
        "UTC",
        &["a/b"],
        enabled_since(DAY0 + 10 * HOUR),
    );
    let f = [facts("a/b", Some(DAY0 - 20 * DAY + 9 * HOUR), false)];
    assert_eq!(
        verdicts(&w, &f, DAY0 + 12 * HOUR),
        vec![WorkflowVerdict::NotDue]
    );
    assert_eq!(
        verdicts(&w, &f, DAY0 + DAY + 9 * HOUR),
        vec![start(DAY0 + DAY + 9 * HOUR)]
    );
}

#[test]
fn a_disabled_workflow_never_fires() {
    let w = workflow(
        "0 9 * * *",
        "UTC",
        &["a/b"],
        WorkflowState::Disabled { reason: None },
    );
    assert!(verdicts(&w, &[facts("a/b", None, false)], DAY0 + 12 * HOUR).is_empty());
}

#[test]
fn each_repo_is_decided_on_its_own_facts() {
    let w = workflow(
        "0 9 * * *",
        "UTC",
        &["a/one", "a/two", "a/three"],
        enabled_since(DAY0 - 30 * DAY),
    );
    let f = [
        facts("a/one", Some(DAY0 + 9 * HOUR), false),
        facts("a/two", Some(DAY0 - DAY + 9 * HOUR), true),
    ];
    let got = workflow_due(&w, &f, ts(DAY0 + 12 * HOUR));
    let repos: Vec<_> = got.iter().map(|v| v.repo.to_string()).collect();
    assert_eq!(repos, ["a/one", "a/two", "a/three"]);
    let verdicts: Vec<_> = got.iter().map(|v| v.verdict).collect();
    assert_eq!(
        verdicts,
        [
            WorkflowVerdict::NotDue,
            WorkflowVerdict::SkipOverlap {
                due_at: ts(DAY0 + 9 * HOUR)
            },
            start(DAY0 + 9 * HOUR),
        ]
    );
}

#[test]
fn the_cron_is_read_in_the_workflow_zone() {
    let w = workflow(
        "0 9 * * *",
        "Asia/Hong_Kong",
        &["a/b"],
        enabled_since(DAY0 - 30 * DAY),
    );
    let f = [facts("a/b", None, false)];
    // 09:00 in Hong Kong (UTC+8) is 01:00 UTC.
    assert_eq!(verdicts(&w, &f, DAY0 + 2 * HOUR), vec![start(DAY0 + HOUR)]);
}

/// Every Start a controller ticking each ten minutes from `from` to `to` would make, applying
/// each to the facts as the store would.
fn firings(w: &Workflow, from: i64, to: i64) -> Vec<i64> {
    let mut last_due = None;
    let mut started = Vec::new();
    let mut now = from;
    while now <= to {
        let f = [facts("a/b", last_due, false)];
        if let Some(WorkflowVerdict::Start { due_at }) = verdicts(w, &f, now).first().copied() {
            started.push(due_at.as_millis());
            last_due = Some(due_at.as_millis());
        }
        now += 600_000;
    }
    started
}

#[test]
fn a_time_in_the_spring_forward_gap_fires_once_just_after_it() {
    // New York skips 02:00-03:00 on 2026-03-08, so 02:30 does not exist that day.
    let from = 1_772_928_000_000; // 2026-03-08 00:00 UTC
    let w = workflow(
        "30 2 * * *",
        "America/New_York",
        &["a/b"],
        enabled_since(from),
    );
    let after_gap = 1_772_953_200_000; // 03:00 EDT
    let next_day = after_gap + DAY - 30 * 60_000; // 02:30 EDT
    assert_eq!(firings(&w, from, from + 2 * DAY), vec![after_gap, next_day]);
}

#[test]
fn a_time_in_the_fall_back_overlap_fires_once_at_its_first_pass() {
    // New York repeats 01:00-02:00 on 2026-11-01: 01:30 is 05:30 UTC and again 06:30 UTC.
    let from = 1_793_491_200_000; // 2026-11-01 00:00 UTC
    let w = workflow(
        "30 1 * * *",
        "America/New_York",
        &["a/b"],
        enabled_since(from),
    );
    assert_eq!(firings(&w, from, from + DAY), vec![1_793_511_000_000]);
}

fn run(r: &str, due: i64, recorded: i64, outcome: WorkflowOutcome) -> WorkflowRun {
    WorkflowRun {
        workflow: WorkflowId::new(1),
        repo: repo(r),
        due_at: ts(due),
        outcome,
        recorded_at: ts(recorded),
    }
}

fn started(r: &str, at: i64, box_id: i64) -> WorkflowRun {
    run(
        r,
        at,
        at,
        WorkflowOutcome::Started {
            box_id: BoxId::new(box_id),
        },
    )
}

fn refused(r: &str, due: i64) -> WorkflowRun {
    run(
        r,
        due,
        due,
        WorkflowOutcome::Refused {
            detail: "full".to_owned(),
        },
    )
}

fn timed_out(r: &str, due: i64) -> WorkflowRun {
    let box_id = BoxId::new(1);
    run(
        r,
        due,
        due,
        WorkflowOutcome::TimedOut {
            box_id,
            ended_at: ts(due + 1),
        },
    )
}

fn finished(r: &str, due: i64) -> WorkflowRun {
    let box_id = BoxId::new(1);
    run(
        r,
        due,
        due,
        WorkflowOutcome::Finished {
            box_id,
            ended_at: ts(due + 1),
        },
    )
}

fn skipped(r: &str, due: i64) -> WorkflowRun {
    run(r, due, due, WorkflowOutcome::SkippedOverlap)
}

#[test]
fn a_started_run_expires_at_its_timeout_counted_from_its_start() {
    let mut w = daily();
    w.timeout = TimeoutMinutes::new(30).unwrap();
    let open = [started("a/b", DAY0, 7)];
    let at = |ms| workflow_expired(&w, &open, ts(ms));
    assert!(at(DAY0 + 30 * 60_000 - 1).is_empty());
    let expiry = WorkflowExpiry {
        repo: repo("a/b"),
        due_at: ts(DAY0),
        box_id: BoxId::new(7),
    };
    assert_eq!(at(DAY0 + 30 * 60_000), std::slice::from_ref(&expiry));
    assert_eq!(at(DAY0 + DAY), [expiry]);
}

#[test]
fn the_timeout_counts_from_the_start_not_the_due_time() {
    let w = daily();
    let late = run(
        "a/b",
        DAY0,
        DAY0 + HOUR,
        WorkflowOutcome::Started {
            box_id: BoxId::new(7),
        },
    );
    let ms = |m: i64| ts(DAY0 + HOUR + m * 60_000);
    assert!(workflow_expired(&w, std::slice::from_ref(&late), ms(179)).is_empty());
    assert_eq!(workflow_expired(&w, &[late], ms(180)).len(), 1);
}

#[test]
fn only_started_runs_expire() {
    let w = daily();
    let runs = [
        finished("a/b", 0),
        timed_out("a/b", 1),
        refused("a/b", 2),
        skipped("a/b", 3),
    ];
    assert!(workflow_expired(&w, &runs, ts(DAY0)).is_empty());
}

fn breaker(runs: &[WorkflowRun]) -> bool {
    workflow_breaker(&daily(), runs).is_some()
}

#[test]
fn three_failures_in_a_row_trip_the_breaker_with_a_reason() {
    let base = DAY0 - 10 * DAY;
    let two = [refused("a/b", base), timed_out("a/b", base + DAY)];
    assert!(!breaker(&two));
    let three = [
        two[0].clone(),
        two[1].clone(),
        refused("a/b", base + 2 * DAY),
    ];
    let got = workflow_breaker(&daily(), &three).unwrap();
    assert_eq!(got.repo, repo("a/b"));
    assert!(got.reason.contains("a/b"), "{}", got.reason);
}

#[test]
fn a_finished_run_resets_the_streak() {
    let base = DAY0 - 10 * DAY;
    let runs = [
        refused("a/b", base),
        refused("a/b", base + DAY),
        finished("a/b", base + 2 * DAY),
        refused("a/b", base + 3 * DAY),
        timed_out("a/b", base + 4 * DAY),
    ];
    assert!(!breaker(&runs));
    let more = [runs.to_vec(), vec![refused("a/b", base + 5 * DAY)]].concat();
    assert!(breaker(&more));
}

#[test]
fn skipped_runs_neither_count_nor_reset_the_streak() {
    let base = DAY0 - 10 * DAY;
    let runs = [
        refused("a/b", base),
        skipped("a/b", base + DAY),
        refused("a/b", base + 2 * DAY),
        skipped("a/b", base + 3 * DAY),
        timed_out("a/b", base + 4 * DAY),
    ];
    assert!(breaker(&runs));
    let only_skips: Vec<_> = (0..5).map(|i| skipped("a/b", base + i * DAY)).collect();
    assert!(!breaker(&only_skips));
}

#[test]
fn the_streak_is_per_repo() {
    let base = DAY0 - 10 * DAY;
    let w = workflow("0 9 * * *", "UTC", &["a/b", "a/c"], enabled_since(base));
    let runs = [
        refused("a/b", base),
        refused("a/c", base + DAY),
        refused("a/b", base + 2 * DAY),
        refused("a/c", base + 3 * DAY),
        refused("a/b", base + 4 * DAY),
    ];
    assert_eq!(workflow_breaker(&w, &runs).unwrap().repo, repo("a/b"));
    assert!(workflow_breaker(&w, &runs[..4]).is_none());
}

#[test]
fn failures_before_the_workflow_was_last_enabled_do_not_count() {
    let base = DAY0 - 10 * DAY;
    let w = workflow(
        "0 9 * * *",
        "UTC",
        &["a/b"],
        enabled_since(base + 2 * DAY + HOUR),
    );
    let runs = [
        refused("a/b", base),
        refused("a/b", base + DAY),
        refused("a/b", base + 2 * DAY),
        refused("a/b", base + 3 * DAY),
        refused("a/b", base + 4 * DAY),
    ];
    assert!(
        workflow_breaker(&w, &runs).is_none(),
        "only two since re-enabling"
    );
    let more = [runs.to_vec(), vec![refused("a/b", base + 5 * DAY)]].concat();
    assert!(workflow_breaker(&w, &more).is_some());
}

#[test]
fn a_disabled_workflow_has_nothing_to_trip() {
    let w = workflow(
        "0 9 * * *",
        "UTC",
        &["a/b"],
        WorkflowState::Disabled { reason: None },
    );
    let runs: Vec<_> = (0..4).map(|i| refused("a/b", i * DAY)).collect();
    assert!(workflow_breaker(&w, &runs).is_none());
}

fn with_prior(prior: Option<PriorRun>) -> WorkflowVerdict {
    let f = [WorkflowRepoFacts {
        repo: repo("a/b"),
        last_due: Some(ts(DAY0 - DAY + 9 * HOUR)),
        prior,
    }];
    verdicts(&daily(), &f, DAY0 + 10 * HOUR)
        .into_iter()
        .next()
        .unwrap()
}

#[test]
fn only_an_unfinished_prior_run_blocks_the_next_firing() {
    let skip = WorkflowVerdict::SkipOverlap {
        due_at: ts(DAY0 + 9 * HOUR),
    };
    let go = start(DAY0 + 9 * HOUR);
    let table = [
        (None, go),
        (Some(PriorRun::Starting), skip),
        (Some(PriorRun::Started), skip),
        (Some(PriorRun::Finished), go),
        (Some(PriorRun::TimedOut { box_open: true }), skip),
        (Some(PriorRun::TimedOut { box_open: false }), go),
    ];
    for (prior, want) in table {
        assert_eq!(with_prior(prior), want, "{prior:?}");
    }
}
