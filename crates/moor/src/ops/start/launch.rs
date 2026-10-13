//! Launches the agent of a Working Task and records how that ended.

use std::path::PathBuf;
use std::time::Duration;

use moor_core::activity::PaneRead;
use moor_core::seam::proc::Deadline;

use moor_core::domain::boxes::BoxState;
use moor_core::domain::decision::{Decision, ExtraEvent, Field, Fields};
use moor_core::domain::task::TaskState;
use moor_core::seam::engine::{EngineError, LaunchReq, SessionRef};
use moor_core::seam::surface::{PaneRef, WorkspaceRef};
use moor_core::view::{AgentLaunchView, LaunchFailure};
use moor_core::{StartReq, TaskId, Warning};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::provision::{nonce, recorded_workdir, reread};
use crate::ops::common::view::rfc3339;
use crate::store::{Store, TaskRow};

/// Lines of a failure's output kept as its tail.
const TAIL_LINES: usize = 10;

/// How long a fresh agent pane is watched for first-run prompts.
const PROMPT_WINDOW: Duration = Duration::from_secs(8);

/// Time between two reads of the pane while watching.
const PROMPT_POLL: Duration = Duration::from_millis(500);

/// Deadline of one pane read while watching.
const PROMPT_READ: Duration = Duration::from_secs(5);

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
    if req.no_agent || row.goal.is_none() || store.live_session(id)?.is_some() {
        return Ok(AgentLaunchView::default());
    }
    let Some(mut launch) = launch_req(cx, &row, setup_exit) else {
        return Ok(AgentLaunchView::default());
    };
    launch.model = model()?;
    begin(cx, store, &row, &launch)
}

/// Launches `launch` and records the session, or the failure, on the Task `row`.
pub(super) fn begin(
    cx: &Cx,
    store: &Store,
    row: &TaskRow,
    launch: &LaunchReq,
) -> Result<AgentLaunchView, CommandError> {
    let id = row.id;
    match cx.engine().launch(launch) {
        Ok(session) => {
            let sref = if session.id.is_empty() {
                format!("moor-{}", id.get())
            } else {
                session.id.clone()
            };
            store.begin_session(id, &sref, session.pane.as_ref().map(PaneRef::as_str))?;
            if row.launch_failure.is_some() {
                record(store, id, Field::Clear, None)?;
            }
            watch_prompts(
                cx,
                store,
                id,
                &SessionRef {
                    id: sref.clone(),
                    ..session
                },
            );
            Ok(AgentLaunchView {
                launched: true,
                session: Some(sref),
                reason: None,
            })
        }
        Err(e) => {
            let failure = failure(cx, &e);
            record(store, id, Field::Set(to_json(&failure)?), Some(&failure))?;
            Ok(AgentLaunchView {
                launched: false,
                session: None,
                reason: Some(failure),
            })
        }
    }
}

/// Reads the fresh agent pane for a bounded window and records a first-run prompt.
///
/// A prompt becomes an `agent_prompt_blocked` event and a warning; a failed read becomes an
/// `agent_prompt_check_failed` event. Nothing here fails the start.
fn watch_prompts(cx: &Cx, store: &Store, id: TaskId, session: &SessionRef) {
    if session.pane.is_none() {
        return;
    }
    let begin = cx.clock().instant();
    loop {
        let probes = cx.engine().probe(session, Deadline(PROMPT_READ));
        let event = match probes.map(|p| p.pane) {
            Ok(PaneRead::Fresh(obs)) => cx.engine().first_run_prompt(&obs.tail).map(|prompt| {
                let name = prompt.name();
                cx.warn(Warning {
                    code: "agent_prompt_blocked".to_owned(),
                    message: format!("the agent waits at a {name} prompt; answer it in its pane"),
                });
                ExtraEvent::AgentPromptBlocked {
                    prompt: name.to_owned(),
                }
            }),
            Ok(PaneRead::Gone | PaneRead::Absent) => return,
            Ok(PaneRead::TimedOut) => Some(ExtraEvent::AgentPromptCheckFailed {
                error: "the pane read failed or timed out".to_owned(),
            }),
            Err(e) => Some(ExtraEvent::AgentPromptCheckFailed {
                error: e.to_string(),
            }),
        };
        if let Some(event) = event {
            if let Err(e) = store.apply(Decision::new(id).event(event)) {
                cx.warn(Warning {
                    code: "agent_prompt_record_failed".to_owned(),
                    message: format!("could not record the first-run prompt check: {e}"),
                });
            }
            return;
        }
        if cx.clock().instant().duration_since(begin) + PROMPT_POLL > PROMPT_WINDOW {
            return;
        }
        cx.clock().sleep(PROMPT_POLL);
    }
}

/// What the Engine needs to launch `row`'s agent; none unless the Task is Working with a ready Box.
pub(super) fn launch_req(cx: &Cx, row: &TaskRow, setup_exit: Option<i32>) -> Option<LaunchReq> {
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
        goal: row.goal.clone(),
        resume: None,
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

/// Writes the Task's launch failure column, with a `session_failed` event for a new failure.
fn record(
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
