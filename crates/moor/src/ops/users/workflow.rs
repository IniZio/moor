//! A user's workflows: `workflow-add`, `workflow-list`, `workflow-remove`,
//! `workflow-set-enabled` and `workflow-runs`.

use moor_core::name::ForgeRepo;
use moor_core::request::{WorkflowAddReq, WorkflowCmd};
use moor_core::seam::clock::Ts;
use moor_core::user::UserId;
use moor_core::view::workflow::{
    ChangeView, LastRunView, OriginView, WorkflowChangeValue, WorkflowListValue, WorkflowRunsValue,
    WorkflowView,
};
use moor_core::workflow::{
    ChangeKind, TimeoutMinutes, Workflow, WorkflowDef, WorkflowName, WorkflowOrigin,
    WorkflowOutcome, WorkflowRun, WorkflowSchedule,
};
use moor_core::ErrorKind;

use super::to_value;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::Store;

/// Changes `workflow-runs` returns when the request names no limit.
const DEFAULT_LIMIT: u32 = 50;

/// Most changes `workflow-runs` returns at once.
const MAX_LIMIT: u32 = 200;

/// Runs `cmd` for `user`.
pub(super) fn run(
    cx: &Cx,
    store: &Store,
    user: UserId,
    cmd: &WorkflowCmd,
) -> Result<serde_json::Value, CommandError> {
    let now = cx.clock().now();
    match cmd {
        WorkflowCmd::Add { req, origin_front } => {
            add(cx, store, user, req, origin_front.as_deref())
        }
        WorkflowCmd::List => list(store, user, now),
        WorkflowCmd::Remove { name } => {
            let name = parse_name(name)?;
            let changed = store.delete_workflow(user, &name)?;
            to_value(&WorkflowChangeValue {
                name: name.to_string(),
                changed,
                next_due: None,
            })
        }
        WorkflowCmd::SetEnabled { name, enabled } => set_enabled(store, user, name, *enabled, now),
        WorkflowCmd::Runs { after, limit } => runs(store, user, *after, *limit),
    }
}

/// A refusal for input the caller can correct.
fn usage(why: impl std::fmt::Display) -> CommandError {
    CommandError::new(ErrorKind::Usage, why.to_string())
}

/// `name` as a workflow name.
fn parse_name(name: &str) -> Result<WorkflowName, CommandError> {
    WorkflowName::parse(name).map_err(usage)
}

/// `workflow-add`: checks every field and says which one is wrong.
fn add(
    cx: &Cx,
    store: &Store,
    user: UserId,
    req: &WorkflowAddReq,
    origin_front: Option<&str>,
) -> Result<serde_json::Value, CommandError> {
    let name = parse_name(&req.name)?;
    let floor = cx.env().workflow_min_interval();
    let schedule = WorkflowSchedule::parse_with_min_interval(&req.cron, &req.timezone, floor)
        .map_err(usage)?;
    let repos = req
        .repos
        .iter()
        .enumerate()
        .map(|(i, r)| {
            ForgeRepo::parse(r).map_err(|_| usage(format!("repos[{i}] must be OWNER/NAME")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let timeout = req
        .timeout_minutes
        .map_or(Ok(TimeoutMinutes::DEFAULT), TimeoutMinutes::new)
        .map_err(usage)?;
    let mut def = WorkflowDef::new(name.clone(), schedule, repos, &req.prompt, (true, timeout))
        .map_err(usage)?;
    def.origin = match (origin_front, &req.origin_key) {
        (Some(front), Some(key)) => Some(WorkflowOrigin {
            front: front.to_owned(),
            key: Some(key.clone()),
        }),
        (None, Some(_)) => {
            return Err(CommandError::new(
                ErrorKind::Refused,
                "only a front can send an origin_key",
            ));
        }
        _ => None,
    };
    store.put_workflow(user, &def)?;
    to_value(&WorkflowChangeValue {
        name: name.to_string(),
        changed: true,
        next_due: def
            .schedule
            .first_after(cx.clock().now())
            .map(Ts::as_millis),
    })
}

/// `workflow-set-enabled`.
fn set_enabled(
    store: &Store,
    user: UserId,
    name: &str,
    enabled: bool,
    now: Ts,
) -> Result<serde_json::Value, CommandError> {
    let name = parse_name(name)?;
    if !store.set_workflow_enabled(user, &name, enabled)? {
        return Err(CommandError::new(
            ErrorKind::Refused,
            format!("no workflow named {name}"),
        ));
    }
    let next_due = match store.workflow(user, &name)? {
        Some(w) if enabled => w.schedule.first_after(now).map(Ts::as_millis),
        _ => None,
    };
    to_value(&WorkflowChangeValue {
        name: name.to_string(),
        changed: true,
        next_due,
    })
}

/// `workflow-list`.
fn list(store: &Store, user: UserId, now: Ts) -> Result<serde_json::Value, CommandError> {
    let mut workflows = Vec::new();
    for w in store.workflows(user)? {
        let mut last_runs = Vec::new();
        for run in store.last_workflow_runs(w.id)? {
            last_runs.push(run_view(store, &run)?);
        }
        workflows.push(view(&w, now, last_runs));
    }
    to_value(&WorkflowListValue { workflows })
}

/// `w` as shown.
fn view(w: &Workflow, now: Ts, last_runs: Vec<LastRunView>) -> WorkflowView {
    let (enabled, disabled_reason) = match &w.state {
        moor_core::workflow::WorkflowState::Enabled { .. } => (true, None),
        moor_core::workflow::WorkflowState::Disabled { reason } => (false, reason.clone()),
    };
    WorkflowView {
        name: w.name.to_string(),
        cron: w.schedule.expr().to_owned(),
        timezone: w.schedule.zone().to_owned(),
        repos: w.repos.iter().map(ToString::to_string).collect(),
        prompt: w.prompt.clone(),
        enabled,
        disabled_reason,
        timeout_minutes: w.timeout.minutes(),
        next_due: enabled
            .then(|| w.schedule.first_after(now).map(Ts::as_millis))
            .flatten(),
        last_runs,
    }
}

/// `run` as shown, naming its Box.
fn run_view(store: &Store, run: &WorkflowRun) -> Result<LastRunView, CommandError> {
    let (outcome, detail) = match &run.outcome {
        WorkflowOutcome::Starting => ("starting", None),
        WorkflowOutcome::Started { .. } => ("started", None),
        WorkflowOutcome::Finished { .. } => ("finished", None),
        WorkflowOutcome::TimedOut { .. } => ("timed_out", None),
        WorkflowOutcome::SkippedOverlap => ("skipped_overlap", None),
        WorkflowOutcome::Refused { detail } => ("refused", Some(detail.clone())),
    };
    let box_name = match run.outcome.box_id() {
        Some(id) => store.box_row(id)?.map(|r| r.name.as_str().to_owned()),
        None => None,
    };
    Ok(LastRunView {
        repo: run.repo.to_string(),
        due_at: run.due_at.as_millis(),
        outcome: outcome.to_owned(),
        detail,
        box_name,
    })
}

/// `workflow-runs`: the changes after the cursor.
fn runs(
    store: &Store,
    user: UserId,
    after: Option<i64>,
    limit: Option<u32>,
) -> Result<serde_json::Value, CommandError> {
    let after = after.unwrap_or(0).max(0);
    let limit = limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let mut changes = Vec::new();
    let mut next = after;
    for change in store.workflow_changes(user, after, u64::from(limit))? {
        next = next.max(change.seq);
        let origin = change.origin.map(|o| OriginView {
            front: o.front,
            key: o.key,
        });
        let workflow = change.workflow.to_string();
        let seq = change.seq;
        changes.push(match change.what {
            ChangeKind::Run(run) => ChangeView::Run {
                seq,
                workflow,
                origin,
                run: run_view(store, &run)?,
            },
            ChangeKind::Disabled { reason } => ChangeView::Disabled {
                seq,
                workflow,
                origin,
                reason,
            },
        });
    }
    to_value(&WorkflowRunsValue { changes, next })
}

#[cfg(test)]
mod tests;
