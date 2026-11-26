//! The reads of workflow runs: the change stream and one workflow's run list.

use std::collections::hash_map::Entry;
use std::collections::HashMap;

use moor_core::gate::Gates;
use moor_core::user::UserId;
use moor_core::view::workflow::{
    ChangeView, HistoryLineView, LastRunView, OriginView, RunRowView, WorkflowRunListValue,
    WorkflowRunReadValue, WorkflowRunsValue,
};
use moor_core::workflow::{box_name, PullTarget, RunEnd, RunOutcome, RunState, WorkflowRun};
use moor_core::ErrorKind;

use super::{parse_name, to_value};
use crate::error::CommandError;
use crate::store::Store;

/// Changes `workflow-runs` returns when the request names no limit.
const DEFAULT_LIMIT: u32 = 50;

/// Runs `workflow-run-list` returns when the request names no limit.
const DEFAULT_RUN_LIST: u32 = 30;

/// Most changes `workflow-runs` returns at once.
const MAX_LIMIT: u32 = 200;

/// The detail and result a run in `state` shows: a refusal's, failure's or incomplete run's
/// reason, a delivery's result.
fn told(state: &RunState) -> (Option<String>, Option<String>) {
    match state {
        RunState::Done(RunEnd::Refused { detail }) => (Some(detail.clone()), None),
        RunState::Done(RunEnd::SkippedFork { author }) => (
            Some(format!(
                "the fork's author {author} cannot write to the repo"
            )),
            None,
        ),
        RunState::Done(RunEnd::Ended { outcome, .. }) => match outcome {
            RunOutcome::Failed { reason, .. } => (Some(reason.clone()), None),
            RunOutcome::Incomplete { at, why } => (Some(why.says(at)), None),
            RunOutcome::Delivered { result, .. } => {
                (None, Some(result.clone()).filter(|r| !r.is_empty()))
            }
            RunOutcome::Declined { .. }
            | RunOutcome::TimedOut { .. }
            | RunOutcome::Cancelled { .. } => (None, None),
        },
        RunState::Queued
        | RunState::Running { .. }
        | RunState::Waiting { .. }
        | RunState::Done(RunEnd::SkippedOverlap | RunEnd::CancelledUnstarted { .. }) => {
            (None, None)
        }
    }
}

/// `run` as shown, naming its Box.
pub(super) fn run_view(
    store: &Store,
    workflow: &str,
    run: &WorkflowRun,
) -> Result<LastRunView, CommandError> {
    let (detail, result) = told(&run.state);
    let existing = match run.state.box_id() {
        Some(id) => store.box_row(id)?.map(|r| r.name.as_str().to_owned()),
        None => None,
    };
    let planned_box = matches!(
        run.state,
        RunState::Queued | RunState::Done(RunEnd::Refused { .. })
    )
    .then(|| box_name(run.workflow, workflow, run.due_at, &run.cause).ok())
    .flatten()
    .map(|name| name.as_str().to_owned());
    let history = store
        .run_history(run.id)?
        .into_iter()
        .map(|line| HistoryLineView {
            at: line.at.as_millis(),
            entry: line.entry,
        })
        .collect();
    Ok(LastRunView {
        due_at: run.due_at.as_millis(),
        outcome: run.state.tag().to_owned(),
        phase: run.state.phase().as_str().to_owned(),
        verdict: match &run.state {
            RunState::Done(RunEnd::Ended {
                outcome: RunOutcome::Delivered { verdict, .. },
                ..
            }) => verdict.clone(),
            _ => None,
        },
        trigger: run.cause.trigger().as_str().to_owned(),
        pull: run
            .cause
            .subject()
            .map(|p| format!("{}#{}", p.repo, p.number)),
        head_sha: run.cause.subject().map(|p| p.head_sha.to_string()),
        step: run.state.step().map(ToString::to_string),
        result,
        detail,
        box_name: existing,
        planned_box,
        history,
    })
}

/// `workflow-runs`: the changes after the cursor, or the latest ones when there is none. The
/// store owns the paging; `next` is the newest change returned, or the cursor again.
pub(super) fn runs(
    store: &Store,
    user: UserId,
    (after, limit): (Option<i64>, Option<u32>),
    (workflow, pull): (Option<&str>, Option<&PullTarget>),
) -> Result<serde_json::Value, CommandError> {
    let only = workflow.map(parse_name).transpose()?;
    let after = after.map(|a| a.max(0));
    let limit = limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let mut changes = Vec::new();
    let mut next = after.unwrap_or(0);
    let mut prompts = HashMap::new();
    for change in store.workflow_changes(user, after, u64::from(limit), (only.as_ref(), pull))? {
        next = next.max(change.seq);
        let step_title = match change.run.state.step() {
            Some(step) => {
                let prompt = match prompts.entry(change.workflow.clone()) {
                    Entry::Occupied(known) => known.into_mut(),
                    Entry::Vacant(slot) => slot.insert(
                        store
                            .workflow(user, &change.workflow)?
                            .map(|w| w.prompt)
                            .unwrap_or_default(),
                    ),
                };
                let plan = store.run_plan(change.run.id, prompt)?;
                plan.step(step).map(|s| s.title.clone())
            }
            None => None,
        };
        let origin = change.origin.map(|o| OriginView {
            front: o.front,
            key: o.key,
        });
        let workflow = change.workflow.to_string();
        let seq = change.seq;
        let run = Box::new(run_view(store, &workflow, &change.run)?);
        changes.push(ChangeView::Run {
            seq,
            workflow,
            id: change.run.id.get(),
            step_title,
            origin,
            run,
        });
    }
    to_value(&WorkflowRunsValue { changes, next })
}

/// The plan document of `run` as a page lists its steps: the gates when its Box has them, or
/// when it has no Box yet and the workflow has them, else the frozen plan.
fn shown_plan(
    store: &Store,
    run: &WorkflowRun,
    current: Option<&Gates>,
    prompt: &str,
) -> Result<serde_json::Value, CommandError> {
    let held = match run.state.box_id() {
        Some(id) => store.box_gates(id)?,
        None => current.cloned(),
    };
    if let Some(gates) = held {
        return Ok(gates.steps_doc());
    }
    serde_json::to_value(store.run_plan(run.id, prompt)?.to_doc())
        .map_err(|e| CommandError::new(ErrorKind::Failed, e.to_string()))
}

/// `workflow-run-read`: run `id` of any workflow with its frozen plan, read by anyone signed in.
pub(super) fn run_read(
    store: &Store,
    user: UserId,
    id: i64,
) -> Result<serde_json::Value, CommandError> {
    let missing = || CommandError::new(ErrorKind::Refused, format!("no run {id}"));
    let run = store.workflow_run_by_id(id)?.ok_or_else(missing)?;
    let w = store.workflow_by_id(run.workflow)?.ok_or_else(missing)?;
    let plan = shown_plan(store, &run, w.gates.as_ref(), &w.prompt)?;
    let view = run_view(store, w.name.as_str(), &run)?;
    to_value(&WorkflowRunReadValue {
        workflow: w.name.to_string(),
        yours: w.owner == user,
        run: RunRowView {
            id: run.id.get(),
            run: view,
            plan: Some(plan),
        },
    })
}

/// `workflow-run-list`: the latest runs of one workflow, or one run with its frozen plan.
pub(super) fn run_list(
    store: &Store,
    user: UserId,
    workflow: &str,
    only: Option<i64>,
    limit: Option<u32>,
) -> Result<serde_json::Value, CommandError> {
    let name = parse_name(workflow)?;
    let Some(w) = store.workflow(user, &name)? else {
        return Err(CommandError::new(
            ErrorKind::Refused,
            format!("no workflow named {name}"),
        ));
    };
    let limit = limit.unwrap_or(DEFAULT_RUN_LIST).clamp(1, MAX_LIMIT);
    let mut runs = Vec::new();
    for run in store.workflow_run_list(w.id, only, u64::from(limit))? {
        let plan = match only {
            Some(_) => Some(shown_plan(store, &run, w.gates.as_ref(), &w.prompt)?),
            None => None,
        };
        let view = run_view(store, name.as_str(), &run)?;
        runs.push(RunRowView {
            id: run.id.get(),
            run: view,
            plan,
        });
    }
    to_value(&WorkflowRunListValue {
        workflow: name.to_string(),
        runs,
    })
}
