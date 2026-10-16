//! Launches the agent of a Working Task and records how that ended.

use std::path::PathBuf;
use std::sync::Arc;

use moor_core::activity::PaneStatus;
use moor_core::config::TaskSettings;
use moor_core::seam::agent_profile::ProxyAddr;
use moor_core::seam::proxy_host::{ProxyEndpoint, ProxySpec};

use moor_core::domain::boxes::BoxState;
use moor_core::domain::decision::{Decision, ExtraEvent, Field, Fields};
use moor_core::domain::task::TaskState;
use moor_core::seam::engine::{EngineError, LaunchReq, SessionRef};
use moor_core::seam::surface::{PaneRef, WorkspaceRef};
use moor_core::view::{AgentLaunchView, LaunchFailure};
use moor_core::{StartReq, TaskId};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::provision::{nonce, recorded_workdir, reread};
use crate::ops::common::proxy::{box_env, origin, plan, ProxyPlan};
use crate::ops::common::resume;
use crate::ops::common::view::rfc3339;
use crate::store::{Store, TaskRow};

/// Lines of a failure's output kept as its tail.
pub(super) const TAIL_LINES: usize = 10;

/// Launches the agent when the Task is Working with a ready Box, a goal and no live session.
///
/// `setup_exit` is the exit code of a setup that just failed; the agent still launches then.
/// A launch that fails is recorded on the Task, which stays Working with no session, and
/// is reported in the view; a later launch that succeeds clears it. `settings` yields the
/// Task's settings, which supply both the model and the proxy's egress.
///
/// # Errors
/// A store failure, or settings that cannot be resolved.
pub(super) fn run(
    cx: &Cx,
    store: &Store,
    req: &StartReq,
    id: TaskId,
    setup_exit: Option<i32>,
    settings: &dyn Fn() -> Result<TaskSettings, CommandError>,
) -> Result<AgentLaunchView, CommandError> {
    let row = reread(store, id)?;
    let resume = row
        .agent_session_id
        .clone()
        .filter(|s| req.resume && req.goal.is_none() && !s.is_empty());
    if req.no_agent || (row.goal.is_none() && resume.is_none()) || store.live_session(id)?.is_some()
    {
        return Ok(AgentLaunchView::default());
    }
    let Some(mut launch) = launch_req(cx, &row, setup_exit) else {
        return Ok(AgentLaunchView::default());
    };
    launch.resume = resume;
    let cfg = settings()?;
    launch.model = cfg.model.clone().map(|m| m.value);
    begin(cx, store, &row, &launch, &cfg)
}

/// Starts or reuses the Task's proxy for `plan`, returning the spec it was given and its endpoint.
fn ensure_proxy(
    cx: &Cx,
    row: &TaskRow,
    launch: &LaunchReq,
    plan: ProxyPlan,
) -> Result<(ProxySpec, ProxyEndpoint), String> {
    let spec = ProxySpec {
        task: row.id,
        box_handle: row.boxed.as_ref().and_then(|b| b.handle.clone()),
        host_cwd: launch.worktree.clone(),
        task_repo: plan.repo,
        rules: plan.rules,
    };
    let ep = cx.proxy_host().ensure(&spec).map_err(|e| e.to_string())?;
    Ok((spec, ep))
}

/// Launches `launch` and records the session, or the failure, on the Task `row`.
///
/// `cfg` supplies the egress the proxy plan allows.
pub(super) fn begin(
    cx: &Cx,
    store: &Store,
    row: &TaskRow,
    launch: &LaunchReq,
    cfg: &TaskSettings,
) -> Result<AgentLaunchView, CommandError> {
    let id = row.id;
    let origin = origin(cx, &launch.worktree);
    let plan = match plan(cx, origin.as_deref(), cfg) {
        Ok(p) => p,
        Err(message) => {
            let e = EngineError::Failed {
                step: "proxy",
                message,
            };
            return record_failure(cx, store, id, &e);
        }
    };
    let repo_env = plan.env.clone();
    let (spec, ep) = match ensure_proxy(cx, row, launch, plan) {
        Ok(pair) => pair,
        Err(message) => {
            let e = EngineError::Failed {
                step: "proxy",
                message,
            };
            return record_failure(cx, store, id, &e);
        }
    };
    let mut launch = launch.clone();
    launch.proxy = Some(Arc::new(ProxyAddr {
        box_port: ep.box_port,
        nonce: ep.nonce,
        ca_pem: ep.ca_pem,
        box_env: merged_env(repo_env, box_env(&spec.rules)),
    }));
    launch.origin = origin;
    let launch = &launch;
    match cx.engine().launch(launch) {
        Ok(session) => record_session(cx, store, row, launch, session),
        Err(e) => record_failure(cx, store, id, &e),
    }
}

/// The repo `env` followed by the rule `env`; a rule's value replaces the repo's of the same name.
fn merged_env(repo: Vec<(String, String)>, rules: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = repo
        .into_iter()
        .filter(|(name, _)| !rules.iter().any(|(r, _)| r == name))
        .collect();
    out.extend(rules);
    out
}

/// Records the launched `session` on the Task `row` and reports it.
fn record_session(
    cx: &Cx,
    store: &Store,
    row: &TaskRow,
    launch: &LaunchReq,
    session: SessionRef,
) -> Result<AgentLaunchView, CommandError> {
    let id = row.id;
    let sandbox = session.sandbox.clone();
    let sref = if session.id.is_empty() {
        format!("moor-{}", id.get())
    } else {
        session.id
    };
    store.begin_session(id, &sref, session.pane.as_ref().map(PaneRef::as_str))?;
    if let Some(live) = store.live_session(id)?.filter(|_| launch.resume.is_some()) {
        resume::ensure(
            cx,
            store,
            &row.name,
            &live,
            launch.resume.as_deref(),
            PaneStatus::Working,
        )?;
    }
    if row.launch_failure.is_some() {
        record(store, id, Field::Clear, None)?;
    }
    Ok(AgentLaunchView {
        launched: true,
        session: Some(sref),
        sandbox,
        reason: None,
    })
}

/// Records a failed launch on the Task, which stays Working with no session, and reports it.
fn record_failure(
    cx: &Cx,
    store: &Store,
    id: TaskId,
    e: &EngineError,
) -> Result<AgentLaunchView, CommandError> {
    let failure = failure(cx, e);
    record(store, id, Field::Set(to_json(&failure)?), Some(&failure))?;
    Ok(AgentLaunchView {
        launched: false,
        session: None,
        sandbox: None,
        reason: Some(failure),
    })
}

/// What the Engine needs to launch `row`'s agent; none unless the Task is Working with a ready Box.
pub(super) fn launch_req(cx: &Cx, row: &TaskRow, setup_exit: Option<i32>) -> Option<LaunchReq> {
    let b = row.boxed.as_ref().filter(|b| b.state == BoxState::Ready)?;
    if row.state != TaskState::Working {
        return None;
    }
    let strip_env = cx
        .config()
        .drivers
        .get(b.provider.as_str())
        .map_or_else(Vec::new, |d| d.strip_env.clone());
    Some(LaunchReq {
        handle: b.handle.clone()?,
        workdir: recorded_workdir(b.workdir.as_deref()?)?,
        worktree: PathBuf::from(row.worktree.as_deref()?),
        workspace: WorkspaceRef::new(row.workspace_id.as_deref()?),
        task: row.name.clone(),
        branch: row.branch.clone().unwrap_or_default(),
        goal: row.goal.clone(),
        resume: None,
        issue: row.issue.clone(),
        setup_exit,
        model: None,
        nonce: nonce(cx.rng()),
        proxy: None,
        origin: None,
        strip_env,
    })
}

/// The failure to record for `e`: the first line is the message, the last lines after it the tail.
fn failure(cx: &Cx, e: &EngineError) -> LaunchFailure {
    let (step, text) = match e {
        EngineError::Failed { step, message } => ((*step).to_owned(), message.clone()),
        other => ("launch".to_owned(), other.to_string()),
    };
    let (message, rest) = text.split_once('\n').unwrap_or((&text, ""));
    let lines: Vec<&str> = rest.lines().collect();
    let keep = if step == "proxy" {
        crate::ops::common::proxy::TAIL_LINES
    } else {
        TAIL_LINES
    };
    let tail = lines
        .iter()
        .skip(lines.len().saturating_sub(keep))
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    LaunchFailure {
        step,
        message: message.to_owned(),
        tail: Some(tail).filter(|t| !t.is_empty()),
        at: rfc3339(cx.clock().now()),
    }
}

/// The failure as the JSON kept on the Task row.
pub(super) fn to_json(f: &LaunchFailure) -> Result<String, CommandError> {
    serde_json::to_string(f).map_err(|e| CommandError::new(moor_core::ErrorKind::Internal, e))
}

/// Writes the Task's launch failure column, with a `session_failed` event for a new failure.
pub(super) fn record(
    store: &Store,
    id: TaskId,
    column: Field<String>,
    failed: Option<&LaunchFailure>,
) -> Result<(), CommandError> {
    let set = Fields {
        launch_failure: column,
        ..Fields::default()
    };
    let mut decision = Decision::new(id).set(set);
    if let Some(f) = failed {
        decision = decision.event(ExtraEvent::SessionFailed {
            step: f.step.clone(),
            message: f.message.clone(),
        });
    }
    store.apply(decision)?;
    Ok(())
}
