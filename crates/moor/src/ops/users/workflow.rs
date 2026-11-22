//! A user's workflows: `workflow-add`, `workflow-list`, `workflow-remove`,
//! `workflow-set-enabled` and `workflow-runs`.

use moor_core::config::override_text;
use moor_core::gate::Gates;
use moor_core::name::ForgeRepo;
use moor_core::request::{WorkflowAddReq, WorkflowCmd};
use moor_core::seam::clock::Ts;
use moor_core::user::UserId;
use moor_core::view::workflow::{
    LastRunView, WorkflowChangeValue, WorkflowListValue, WorkflowView,
};
use moor_core::workflow::{
    Plan, PlanError, PromptHash, TimeoutMinutes, Workflow, WorkflowDef, WorkflowName,
    WorkflowOrigin, WorkflowSchedule, WorkflowState,
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

/// The schedule `req` asks for on top of `prior`, the workflow it replaces: a cron left out is
/// the stored one, and a blank cron is none, which fires only by hand or on a pull request. A
/// cron needs its zone, and one left out is the stored one.
fn parse_schedule(
    req: &WorkflowAddReq,
    prior: Option<&Workflow>,
    floor: std::time::Duration,
) -> Result<Option<WorkflowSchedule>, CommandError> {
    let stored = prior.and_then(|w| w.schedule.as_ref());
    let cron = req
        .cron
        .as_deref()
        .or_else(|| stored.map(WorkflowSchedule::expr));
    let Some(cron) = cron.map(str::trim).filter(|c| !c.is_empty()) else {
        return Ok(None);
    };
    let zone = req
        .timezone
        .as_deref()
        .or_else(|| stored.map(WorkflowSchedule::zone))
        .map(str::trim)
        .filter(|z| !z.is_empty())
        .ok_or_else(|| usage("a cron needs a timezone"))?;
    WorkflowSchedule::parse_with_min_interval(cron, zone, floor)
        .map(Some)
        .map_err(usage)
}

/// The first firing of `schedule` after `now`, in Unix milliseconds; none with no schedule.
fn next_due(schedule: Option<&WorkflowSchedule>, now: Ts) -> Option<i64> {
    schedule?.first_after(now).map(Ts::as_millis)
}

/// The fields of an add that fall back to the stored workflow when the request leaves them out.
struct Kept {
    /// Minutes a run may take.
    timeout: TimeoutMinutes,
    /// Whether it fires.
    enabled: bool,
    /// The partial repo config.
    config: Option<String>,
    /// Whether a pull request fires a run.
    on_pull_request: bool,
}

impl Kept {
    /// `req`'s values, with `prior`'s, or the defaults of a new workflow, for what it leaves out.
    fn of(req: &WorkflowAddReq, prior: Option<&Workflow>) -> Result<Self, CommandError> {
        let timeout = match (req.timeout_minutes, prior) {
            (Some(minutes), _) => TimeoutMinutes::new(minutes).map_err(usage)?,
            (None, Some(w)) => w.timeout,
            (None, None) => TimeoutMinutes::DEFAULT,
        };
        Ok(Self {
            timeout,
            enabled: req.enabled.unwrap_or_else(|| {
                prior.is_none_or(|w| matches!(w.state, WorkflowState::Enabled { .. }))
            }),
            config: match (&req.config, prior) {
                (None, Some(w)) => w.config.clone(),
                (config, _) => override_text(config.clone()),
            },
            on_pull_request: req
                .on_pull_request
                .unwrap_or_else(|| prior.is_some_and(|w| w.on_pull_request)),
        })
    }
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
    let prior = store.workflow(user, &name)?;
    let schedule = parse_schedule(req, prior.as_ref(), floor)?;
    let repos = parse_repos("repos", &req.repos)?;
    box_config::check(req.config.as_deref())?;
    let Kept {
        timeout,
        enabled,
        config,
        on_pull_request,
    } = Kept::of(req, prior.as_ref())?;
    let mut def = WorkflowDef::new(
        name.clone(),
        schedule,
        repos,
        &req.prompt,
        (enabled, timeout),
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
    def.config = config;
    def.on_pull_request = on_pull_request;
    (def.plan, def.gates) = plan_or_gates(req, prior.as_ref())?;
    let revision = match store.put_workflow_at(
        user,
        &def,
        req.expected_revision,
        req.keep_state || req.enabled.is_none(),
    )? {
        Put::Written { revision, .. } => revision,
        Put::Stale { current } => return Err(stale(&name, current, req.expected_revision)),
    };
    to_value(&WorkflowChangeValue {
        name: name.to_string(),
        changed: true,
        next_due: def
            .enabled
            .then(|| next_due(def.schedule.as_ref(), cx.clock().now()))
            .flatten(),
        revision: Some(revision),
        plan: def.plan.as_ref().map(Plan::outline).unwrap_or_default(),
    })
}

/// The gates `req` asks for on top of `prior`, the workflow it replaces: gates or outcomes left
/// out are the stored ones, and an empty list is none.
fn parse_gates(
    req: &WorkflowAddReq,
    prior: Option<&Workflow>,
) -> Result<Option<Gates>, CommandError> {
    let outcomes = req.outcomes.as_deref().map(Vec::as_slice);
    match req.gates.as_deref() {
        None if outcomes.is_some() => Err(usage(
            "send outcomes with the gates they end, so nothing was saved".to_owned(),
        )),
        None => Ok(prior.and_then(|w| w.gates.clone())),
        Some([]) if outcomes.is_some_and(|o| !o.is_empty()) => Err(usage(
            "outcomes need gates: a workflow without gates ends as its plan says, so nothing was \
             saved"
                .to_owned(),
        )),
        Some([]) => Ok(None),
        Some(docs) => {
            let stored = || {
                prior
                    .and_then(|w| w.gates.as_ref())
                    .map(Gates::outcome_docs)
                    .unwrap_or_default()
            };
            let outcomes = outcomes.map_or_else(stored, <[_]>::to_vec);
            Gates::parse(docs, &outcomes)
                .map(Some)
                .map_err(|e| usage(e.to_string()))
        }
    }
}

/// The plan and the gates the workflow ends up with, refused when it would have both.
fn plan_or_gates(
    req: &WorkflowAddReq,
    prior: Option<&Workflow>,
) -> Result<(Option<Plan>, Option<Gates>), CommandError> {
    let plan = req
        .plan
        .as_deref()
        .map(|text| parse_plan(text, &req.prompt))
        .transpose()?;
    let gates = parse_gates(req, prior)?;
    if plan.is_some() && gates.is_some() {
        return Err(usage(if req.gates.is_some() {
            "a workflow has a plan or gates, not both: send one of them, so nothing was saved"
        } else {
            "this workflow keeps its gates, and a workflow has a plan or gates, not both: send `gates: []` to clear the gates, or leave the plan out, so nothing was saved"
        }));
    }
    Ok((plan, gates))
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
        Some(w) if enabled => next_due(w.schedule.as_ref(), now),
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
    let enabled = matches!(w.state, moor_core::workflow::WorkflowState::Enabled { .. });
    WorkflowView {
        name: w.name.to_string(),
        cron: w.schedule.as_ref().map(|s| s.expr().to_owned()),
        timezone: w.schedule.as_ref().map(|s| s.zone().to_owned()),
        repos: w.repos.iter().map(ToString::to_string).collect(),
        config: w.config.clone(),
        prompt: w.prompt.clone(),
        enabled,
        timeout_minutes: w.timeout.minutes(),
        next_due: enabled
            .then(|| next_due(w.schedule.as_ref(), now))
            .flatten(),
        plan: (!w.plan.is_single_of(&w.prompt))
            .then(|| serde_json::to_value(w.plan.to_doc()).ok())
            .flatten(),
        plan_stale: w.plan.made_from != PromptHash::of(&w.prompt),
        revision: w.revision,
        on_pull_request: w.on_pull_request,
        gates: w.gates.as_ref().map(Gates::docs),
        outcomes: w
            .gates
            .as_ref()
            .map(Gates::outcome_docs)
            .filter(|o| !o.is_empty()),
        last_run,
    }
}

mod stream;
#[cfg(test)]
mod tests;
