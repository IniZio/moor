//! A user's workflows: `workflow-add`, `workflow-list`, `workflow-remove`,
//! `workflow-set-enabled` and `workflow-runs`.

use moor_core::config::override_text;
use moor_core::name::ForgeRepo;
use moor_core::request::{WorkflowAddReq, WorkflowCmd};
use moor_core::seam::clock::Ts;
use moor_core::user::UserId;
use moor_core::view::workflow::{
    ChangeView, HistoryLineView, LastRunView, OriginView, WorkflowChangeValue, WorkflowListValue,
    WorkflowRunsValue, WorkflowView,
};
use moor_core::workflow::{
    box_name, ChangeKind, Plan, PlanError, PromptHash, RunEnd, RunOutcome, RunState,
    TimeoutMinutes, Workflow, WorkflowDef, WorkflowName, WorkflowOrigin, WorkflowRun,
    WorkflowSchedule,
};
use moor_core::ErrorKind;

use super::to_value;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::box_config;
use crate::store::{Put, Store};

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
                revision: None,
                plan: Vec::new(),
            })
        }
        WorkflowCmd::SetEnabled { name, enabled } => set_enabled(store, user, name, *enabled, now),
        WorkflowCmd::Answer {
            workflow,
            repo,
            answer,
            note,
        } => to_value(&crate::ops::workflow::answer_run(
            cx,
            store,
            user,
            (workflow, repo, *answer, note.as_deref()),
        )?),
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

/// `field`'s repos, each `OWNER/NAME`.
fn parse_repos(field: &str, repos: &[String]) -> Result<Vec<ForgeRepo>, CommandError> {
    repos
        .iter()
        .enumerate()
        .map(|(i, r)| {
            ForgeRepo::parse(r).map_err(|_| usage(format!("{field}[{i}] must be OWNER/NAME")))
        })
        .collect()
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
    let repos = parse_repos("repos", &req.repos)?;
    let also = parse_repos("also", &req.also)?;
    box_config::check(req.config.as_deref())?;
    let timeout = req
        .timeout_minutes
        .map_or(Ok(TimeoutMinutes::DEFAULT), TimeoutMinutes::new)
        .map_err(usage)?;
    let mut def = WorkflowDef::new(name.clone(), schedule, repos, &req.prompt, (true, timeout))
        .and_then(|d| d.with_also(also))
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
    def.config = override_text(req.config.clone());
    def.plan = req
        .plan
        .as_deref()
        .map(|text| parse_plan(text, &req.prompt))
        .transpose()?;
    let revision = match store.put_workflow_at(user, &def, req.expected_revision)? {
        Put::Written { revision, .. } => revision,
        Put::Stale { current } => return Err(stale(&name, current, req.expected_revision)),
    };
    to_value(&WorkflowChangeValue {
        name: name.to_string(),
        changed: true,
        next_due: def
            .schedule
            .first_after(cx.clock().now())
            .map(Ts::as_millis),
        revision: Some(revision),
        plan: def.plan.as_ref().map(Plan::outline).unwrap_or_default(),
    })
}

/// The plan in `text`, made for `prompt`, or a refusal that lists every error in it.
fn parse_plan(text: &str, prompt: &str) -> Result<Plan, CommandError> {
    let refuse = |errors: Vec<PlanError>| {
        let lines: Vec<String> = errors.iter().map(|e| format!("- {e}")).collect();
        usage(format!(
            "the plan has {} error{}, so nothing was saved:\n{}",
            lines.len(),
            if lines.len() == 1 { "" } else { "s" },
            lines.join("\n")
        ))
    };
    Plan::parse(text, prompt).map_err(refuse)
}

/// The refusal for a write whose `expected_revision` is not the stored one.
fn stale(name: &WorkflowName, current: Option<u64>, wanted: Option<u64>) -> CommandError {
    let now = current.map_or_else(
        || "does not exist".to_owned(),
        |r| format!("is at revision {r}"),
    );
    CommandError::new(
        ErrorKind::Refused,
        format!(
            "workflow {name} {now}, not revision {}: it changed after you read it",
            wanted.unwrap_or_default()
        ),
    )
    .with_hint("read it again with list_workflows, apply your change to what it shows, and add it with that revision")
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
        revision: None,
        plan: Vec::new(),
    })
}

/// `workflow-list`.
fn list(store: &Store, user: UserId, now: Ts) -> Result<serde_json::Value, CommandError> {
    let mut workflows = Vec::new();
    for w in store.workflows(user)? {
        let mut last_runs = Vec::new();
        for run in store.last_workflow_runs(w.id)? {
            last_runs.push(run_view(store, w.name.as_str(), &run)?);
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
        also: w.also.iter().map(ToString::to_string).collect(),
        config: w.config.clone(),
        prompt: w.prompt.clone(),
        enabled,
        disabled_reason,
        timeout_minutes: w.timeout.minutes(),
        next_due: enabled
            .then(|| w.schedule.first_after(now).map(Ts::as_millis))
            .flatten(),
        plan: (!w.plan.is_single_of(&w.prompt))
            .then(|| serde_json::to_value(w.plan.to_doc()).ok())
            .flatten(),
        plan_stale: w.plan.made_from != PromptHash::of(&w.prompt),
        revision: w.revision,
        last_runs,
    }
}

/// `run` as shown, naming its Box.
fn run_view(store: &Store, workflow: &str, run: &WorkflowRun) -> Result<LastRunView, CommandError> {
    let (detail, result) = match &run.state {
        RunState::Done(RunEnd::Refused { detail }) => (Some(detail.clone()), None),
        RunState::Done(RunEnd::Ended { outcome, .. }) => match outcome {
            RunOutcome::Failed { reason, .. } => (Some(reason.clone()), None),
            RunOutcome::Delivered { result } => {
                (None, Some(result.clone()).filter(|r| !r.is_empty()))
            }
            RunOutcome::Incomplete { .. }
            | RunOutcome::Declined { .. }
            | RunOutcome::TimedOut { .. } => (None, None),
        },
        RunState::Queued
        | RunState::Running { .. }
        | RunState::Waiting { .. }
        | RunState::Done(RunEnd::SkippedOverlap) => (None, None),
    };
    let existing = match run.state.box_id() {
        Some(id) => store.box_row(id)?.map(|r| r.name.as_str().to_owned()),
        None => None,
    };
    let planned_box = matches!(
        run.state,
        RunState::Queued | RunState::Done(RunEnd::Refused { .. })
    )
    .then(|| box_name(run.workflow, workflow, &run.repo, run.due_at).ok())
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
        repo: run.repo.to_string(),
        due_at: run.due_at.as_millis(),
        outcome: run.state.tag().to_owned(),
        phase: run.state.phase().as_str().to_owned(),
        step: run.state.step().map(ToString::to_string),
        result,
        detail,
        box_name: existing,
        planned_box,
        history,
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
            ChangeKind::Run(run) => {
                let run = run_view(store, &workflow, &run)?;
                ChangeView::Run {
                    seq,
                    workflow,
                    origin,
                    run,
                }
            }
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
