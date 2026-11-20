//! A user's workflows: `workflow-add`, `workflow-list`, `workflow-remove`,
//! `workflow-set-enabled` and `workflow-runs`.

use moor_core::config::override_text;
use moor_core::name::ForgeRepo;
use moor_core::request::{WorkflowAddReq, WorkflowCmd};
use moor_core::seam::clock::Ts;
use moor_core::user::UserId;
use moor_core::view::workflow::{
    LastRunView, WorkflowChangeValue, WorkflowListValue, WorkflowView,
};
use moor_core::workflow::{
    Plan, PlanError, PromptHash, TimeoutMinutes, Workflow, WorkflowDef, WorkflowName,
    WorkflowOrigin, WorkflowSchedule,
};
use moor_core::ErrorKind;

use self::stream::{run_list, run_view, runs};
use super::to_value;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::box_config;
use crate::store::{Put, Store};

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
            pull,
            answer,
            note,
        } => to_value(&crate::ops::workflow::answer_run(
            cx,
            store,
            user,
            (workflow, pull.as_ref()),
            (*answer, note.as_deref()),
        )?),
        WorkflowCmd::RunNow {
            name,
            expected_revision,
            pull,
        } => to_value(&crate::ops::workflow::run_now(
            cx,
            store,
            user,
            (name, *expected_revision, pull.as_ref()),
        )?),
        WorkflowCmd::Runs {
            after,
            limit,
            workflow,
        } => runs(store, user, (*after, *limit), workflow.as_deref()),
        WorkflowCmd::RunList {
            workflow,
            run,
            limit,
        } => run_list(store, user, workflow, *run, *limit),
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
    box_config::check(req.config.as_deref())?;
    let timeout = req
        .timeout_minutes
        .map_or(Ok(TimeoutMinutes::DEFAULT), TimeoutMinutes::new)
        .map_err(usage)?;
    let mut def = WorkflowDef::new(
        name.clone(),
        schedule,
        repos,
        &req.prompt,
        (req.enabled, timeout),
    )
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
    def.on_pull_request = req.on_pull_request;
    def.plan = req
        .plan
        .as_deref()
        .map(|text| parse_plan(text, &req.prompt))
        .transpose()?;
    let revision = match store.put_workflow_at(user, &def, req.expected_revision, req.keep_state)? {
        Put::Written { revision, .. } => revision,
        Put::Stale { current } => return Err(stale(&name, current, req.expected_revision)),
    };
    to_value(&WorkflowChangeValue {
        name: name.to_string(),
        changed: true,
        next_due: def
            .enabled
            .then(|| {
                def.schedule
                    .first_after(cx.clock().now())
                    .map(Ts::as_millis)
            })
            .flatten(),
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
        let last_run = match store.last_workflow_run(w.id)? {
            Some(run) => Some(run_view(store, w.name.as_str(), &run)?),
            None => None,
        };
        workflows.push(view(&w, now, last_run));
    }
    to_value(&WorkflowListValue { workflows })
}

/// `w` as shown.
fn view(w: &Workflow, now: Ts, last_run: Option<LastRunView>) -> WorkflowView {
    let (enabled, disabled_reason) = match &w.state {
        moor_core::workflow::WorkflowState::Enabled { .. } => (true, None),
        moor_core::workflow::WorkflowState::Disabled { reason } => (false, reason.clone()),
    };
    WorkflowView {
        name: w.name.to_string(),
        cron: w.schedule.expr().to_owned(),
        timezone: w.schedule.zone().to_owned(),
        repos: w.repos.iter().map(ToString::to_string).collect(),
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
        on_pull_request: w.on_pull_request,
        last_run,
    }
}

mod stream;
#[cfg(test)]
mod tests;
