//! Launches the agent of a Working Task and records how that ended.

use std::path::PathBuf;
use std::sync::Arc;

use moor_core::activity::PaneStatus;
use moor_core::box_key::BoxKey;
use moor_core::config::{BoxSettings, Flags};

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::boxes::BoxState;
use moor_core::domain::decision::{Decision, ExtraEvent, Field, Fields};
use moor_core::domain::session_state::SessionState;
use moor_core::seam::engine::{EngineError, LaunchPrompt, LaunchReq, SessionRef};
use moor_core::seam::surface::{PaneRef, WorkspaceRef};
use moor_core::view::{AgentLaunchView, LaunchFailure};
use moor_core::{BoxId, StartReq, Warning};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::box_exec;
use crate::ops::common::facts::settings;
use crate::ops::common::provision::{nonce, recorded_workdir, reread};
use crate::ops::common::resume;
use crate::ops::common::view::rfc3339;
use crate::ops::show::redact_nonce;
use crate::store::lease::{LeaseId, LeaseKind};
use crate::store::{BoxRow, Store};

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
    id: BoxId,
    setup_exit: Option<i32>,
    settings: &dyn Fn() -> Result<BoxSettings, CommandError>,
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
        return not_working(cx, store, &row, req.hold);
    };
    launch.resume = resume;
    if req.hold {
        launch.prompt = LaunchPrompt::None;
    }
    let cfg = settings()?;
    launch.model = cfg.model.clone().map(|m| m.value);
    begin(cx, store, &row, &launch, &cfg)
}

/// Records why an agent that should launch on a ready Box did not, unless the Task is held.
fn not_working(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    hold: bool,
) -> Result<AgentLaunchView, CommandError> {
    let ready = row
        .boxed
        .as_ref()
        .is_some_and(|b| b.state == BoxState::Ready);
    if hold || !ready || (row.lifecycle == BoxLifecycle::Ready && !row.session.is_quiet()) {
        return Ok(AgentLaunchView::default());
    }
    let e = EngineError::Failed {
        step: "launch",
        message: format!(
            "the Box is {} with session {}, so the agent was not launched",
            row.lifecycle.as_str(),
            row.session.as_str()
        ),
    };
    record_failure(cx, store, row.id, &e)
}

/// Launches the agent of `row` with `seed` as its first prompt, on a Session the caller opened.
///
/// # Errors
/// A store failure, or settings that cannot be resolved.
pub(crate) fn begin_seeded(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    seed: String,
) -> Result<AgentLaunchView, CommandError> {
    let mut working = row.clone();
    working.lifecycle = BoxLifecycle::Ready;
    working.session = SessionState::Working;
    let Some(mut launch) = launch_req(cx, &working, None) else {
        return Ok(AgentLaunchView::default());
    };
    let at = row.worktree.as_deref().unwrap_or(&row.repo);
    let cfg = settings(cx, std::path::Path::new(at), &Flags::default())?;
    launch.model = cfg.model.clone().map(|m| m.value);
    launch.prompt = LaunchPrompt::Text(seed);
    begin(cx, store, &working, &launch, &cfg)
}

/// Launches `launch` and records the session, or the failure, on the Task `row`.
///
/// `cfg` supplies the egress the proxy plan allows.
pub(super) fn begin(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    launch: &LaunchReq,
    cfg: &BoxSettings,
) -> Result<AgentLaunchView, CommandError> {
    let id = row.id;
    let handle = row.boxed.as_ref().and_then(|b| b.handle.clone());
    let mut proxy = match box_exec::proxy(cx, id, handle, &launch.worktree, cfg) {
        Ok(p) => p,
        Err(message) => {
            let e = EngineError::Failed {
                step: "proxy",
                message,
            };
            return record_failure(cx, store, id, &e);
        }
    };
    // Only the agent gets the filtered repo env; setup and run keep the full one.
    proxy.addr.box_env = moor_core::exec_env::merged_env(
        crate::ops::common::proxy::agent_env(cx, cfg),
        proxy.rule_env.clone(),
    );
    let mut launch = launch.clone();
    launch.proxy = Some(Arc::new(proxy.addr));
    launch.origin = proxy.origin;
    let launch = &launch;
    let leases = match plan_dirs(cx, store, id, launch)? {
        Ok(leases) => leases,
        Err(e) => return record_failure(cx, store, id, &e),
    };
    match cx.engine().launch(launch) {
        Ok(mut session) => {
            for lease in leases {
                store.hold_lease(lease)?;
            }
            for warning in std::mem::take(&mut session.warnings) {
                cx.warn(Warning {
                    code: warning.code,
                    message: warning.message,
                });
            }
            record_session(cx, store, row, launch, session)
        }
        Err(e) => record_failure(cx, store, id, &e),
    }
}

/// Plans a lease for each directory `launch` will create on the Box, before it creates them.
///
/// `Ok(None)` from the Engine means it creates no directories, so none are planned. An Engine
/// error fails the launch before any directory is created, and is returned for the caller to record.
fn plan_dirs(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    launch: &LaunchReq,
) -> Result<Result<Vec<LeaseId>, EngineError>, CommandError> {
    let (cred, tmp) = match cx.engine().launch_dirs(launch) {
        Ok(Some(dirs)) => dirs,
        Ok(None) => return Ok(Ok(Vec::new())),
        Err(e) => return Ok(Err(e)),
    };
    Ok(Ok(vec![
        store.plan_lease(id, LeaseKind::BoxDir, cred.as_str())?,
        store.plan_lease(id, LeaseKind::BoxDir, tmp.as_str())?,
    ]))
}

/// Records the launched `session` on the Task `row` and reports it.
fn record_session(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    launch: &LaunchReq,
    session: SessionRef,
) -> Result<AgentLaunchView, CommandError> {
    let id = row.id;
    let sref = if session.id.is_empty() {
        BoxKey::new(id).session_ref()
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
        reason: None,
    })
}

/// Records a failed launch on the Task, which stays Working with no session, and reports it.
fn record_failure(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    e: &EngineError,
) -> Result<AgentLaunchView, CommandError> {
    let mut failure = failure(cx, e);
    // A proxy log tail can quote the proxy URL, whose userinfo is the nonce.
    let nonce = store.proxy(id).ok().flatten();
    let nonce = nonce.as_ref().map(|r| r.nonce.expose());
    failure.message = redact_nonce(&failure.message, nonce);
    failure.tail = failure.tail.map(|t| redact_nonce(&t, nonce));
    record(store, id, Field::Set(to_json(&failure)?), Some(&failure))?;
    Ok(AgentLaunchView {
        launched: false,
        session: None,
        reason: Some(failure),
    })
}

/// Whether `row` is Ready and its session still has work to do.
pub(super) fn working(row: &BoxRow) -> bool {
    row.lifecycle == BoxLifecycle::Ready && !row.session.is_quiet()
}

/// What the Engine needs to launch `row`'s agent; none unless the Task is Working with a ready Box.
pub(super) fn launch_req(cx: &Cx, row: &BoxRow, setup_exit: Option<i32>) -> Option<LaunchReq> {
    let b = row.boxed.as_ref().filter(|b| b.state == BoxState::Ready)?;
    if !working(row) {
        return None;
    }
    let strip_env = box_exec::strip_env(cx, b.provider.as_str());
    Some(LaunchReq {
        id: row.id,
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
        prompt: LaunchPrompt::Brief,
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
    id: BoxId,
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
