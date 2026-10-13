//! Launches the agent of a Working Task and records how that ended.

use std::path::PathBuf;

use moor_core::domain::boxes::BoxState;
use moor_core::domain::decision::{Decision, Field, Fields};
use moor_core::domain::task::TaskState;
use moor_core::seam::engine::{EngineError, LaunchReq};
use moor_core::seam::surface::{PaneRef, WorkspaceRef};
use moor_core::view::{AgentLaunchView, LaunchFailure};
use moor_core::{StartReq, TaskId};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::provision::{nonce, recorded_workdir, reread};
use crate::ops::common::view::rfc3339;
use crate::store::{Store, TaskRow};

/// Lines of a failure's output kept as its tail.
const TAIL_LINES: usize = 10;

/// Launches the agent when the Task is Working with a ready Box, a goal and no live session.
///
/// `setup_exit` is the exit code of a setup that just failed; the agent still launches then.
/// A launch that fails is recorded on the Task, which stays Working with no session, and
/// is reported in the view; a later launch that succeeds clears it.
///
/// # Errors
/// A store failure, or a model that cannot be resolved.
pub(super) fn run(
    cx: &Cx,
    store: &Store,
    req: &StartReq,
    id: TaskId,
    setup_exit: Option<i32>,
    model: &dyn Fn() -> Result<Option<String>, CommandError>,
) -> Result<AgentLaunchView, CommandError> {
    let row = reread(store, id)?;
    if req.no_agent || store.live_session(id)?.is_some() {
        return Ok(AgentLaunchView::default());
    }
    let Some(mut launch) = launch_req(cx, &row, setup_exit) else {
        return Ok(AgentLaunchView::default());
    };
    launch.model = model()?;
    match cx.engine().launch(&launch) {
        Ok(session) => {
            let sref = if session.id.is_empty() {
                format!("moor-{}", id.get())
            } else {
                session.id
            };
            store.begin_session(id, &sref, session.pane.as_ref().map(PaneRef::as_str))?;
            if row.launch_failure.is_some() {
                record(store, id, Field::Clear)?;
            }
            Ok(AgentLaunchView {
                launched: true,
                session: Some(sref),
                reason: None,
            })
        }
        Err(e) => {
            let failure = failure(cx, &e);
            record(store, id, Field::Set(to_json(&failure)?))?;
            Ok(AgentLaunchView {
                launched: false,
                session: None,
                reason: Some(failure),
            })
        }
    }
}

/// What the Engine needs to launch `row`'s agent; none unless the launch conditions hold.
fn launch_req(cx: &Cx, row: &TaskRow, setup_exit: Option<i32>) -> Option<LaunchReq> {
    let b = row.boxed.as_ref().filter(|b| b.state == BoxState::Ready)?;
    if row.state != TaskState::Working {
        return None;
    }
    Some(LaunchReq {
        handle: b.handle.clone()?,
        workdir: recorded_workdir(b.workdir.as_deref()?)?,
        worktree: PathBuf::from(row.worktree.as_deref()?),
        workspace: WorkspaceRef::new(row.workspace_id.as_deref()?),
        task: row.name.clone(),
        branch: row.branch.clone().unwrap_or_default(),
        goal: Some(row.goal.clone()?),
        issue: row.issue.clone(),
        setup_exit,
        model: None,
        nonce: nonce(cx.rng()),
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
    let tail = lines
        .iter()
        .skip(lines.len().saturating_sub(TAIL_LINES))
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
fn to_json(f: &LaunchFailure) -> Result<String, CommandError> {
    serde_json::to_string(f).map_err(|e| CommandError::new(moor_core::ErrorKind::Internal, e))
}

/// Writes the Task's launch failure column.
fn record(store: &Store, id: TaskId, failure: Field<String>) -> Result<(), CommandError> {
    let set = Fields {
        launch_failure: failure,
        ..Fields::default()
    };
    store.apply(Decision::new(id).set(set))?;
    Ok(())
}
