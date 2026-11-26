//! A user's workflows: `workflow-add`, `workflow-list`, `workflow-remove`,
//! `workflow-set-enabled` and `workflow-runs`.

use moor_core::config::override_text;
use moor_core::gate::Gates;
use moor_core::name::ForgeRepo;
use moor_core::request::{WorkflowAddReq, WorkflowCmd};
use moor_core::seam::clock::Ts;
use moor_core::user::UserId;
use moor_core::view::workflow::WorkflowChangeValue;
use moor_core::workflow::{
    Plan, TimeoutMinutes, Workflow, WorkflowDef, WorkflowName, WorkflowOrigin, WorkflowSchedule,
    WorkflowState,
};
use moor_core::ErrorKind;

use self::list::list;
use self::stream::{run_list, run_read, runs};
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
            pull,
        } => runs(
            store,
            user,
            (*after, *limit),
            (workflow.as_deref(), pull.as_ref()),
        ),
        WorkflowCmd::RunList {
            workflow,
            run,
            limit,
        } => run_list(store, user, workflow, *run, *limit),
        WorkflowCmd::RunRead { run } => run_read(store, user, *run),
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
    def.origin = origin_of(origin_front, req.origin_key.as_deref())?;
    def.config = config;
    def.on_pull_request = on_pull_request;
    (def.plan, def.gates) = kept_reading(prior.as_ref());
    let wants_plan = reading_wanted(req, prior.as_ref())
        .filter(|_| crate::ops::workflow::unreportable(cx, "the plan").is_none());
    let revision = match store.put_workflow_at(
        user,
        &def,
        req.expected_revision,
        req.keep_state || req.enabled.is_none(),
    )? {
        Put::Written { id, revision } => {
            if let Some(replace) = wants_plan {
                store.want_plan(id, &req.prompt, replace)?;
            }
            revision
        }
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

/// The plan and gates a replace keeps: moor's reading of the stored workflow, which a request
/// never sends. A plan made for another prompt is kept, and reads as stale until moor makes it
/// again. A new workflow has neither.
fn kept_reading(prior: Option<&Workflow>) -> (Option<Plan>, Option<Gates>) {
    prior.map_or((None, None), |w| {
        let plan = (!w.plan.is_single_of(&w.prompt)).then(|| w.plan.clone());
        (plan, w.gates.clone())
    })
}

/// The origin a write from the front socket `front` with `key` records; only a front may send a key.
fn origin_of(
    front: Option<&str>,
    key: Option<&str>,
) -> Result<Option<WorkflowOrigin>, CommandError> {
    match (front, key) {
        (Some(front), Some(key)) => Ok(Some(WorkflowOrigin {
            front: front.to_owned(),
            key: Some(key.to_owned()),
        })),
        (None, Some(_)) => Err(CommandError::new(
            ErrorKind::Refused,
            "only a front can send an origin_key",
        )),
        _ => Ok(None),
    }
}

/// Whether the write asks moor to read the prompt again, and if so whether that reading replaces
/// gates a person wrote. A new workflow, a changed prompt, or a request to make it again asks for
/// one, whose plan or gates replace the ones moor made. Gates a person wrote are kept through a
/// prompt change; only a request to make it again replaces them, once moor's reading is stored.
/// A controller whose Box agents cannot report steps asks for none, since it could not run a
/// planned workflow; its runs deliver the prompt in one step.
fn reading_wanted(req: &WorkflowAddReq, prior: Option<&Workflow>) -> Option<bool> {
    let by_hand = prior
        .and_then(|w| w.gates.as_ref())
        .is_some_and(|g| g.made_from().is_none());
    if by_hand {
        return req.regenerate_plan.then_some(true);
    }
    (req.regenerate_plan || prior.is_none_or(|w| w.prompt != req.prompt)).then_some(false)
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

mod list;
mod reading;
mod stream;
#[cfg(test)]
mod tests;
