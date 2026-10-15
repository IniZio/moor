//! Starts the agent again for a Working Task whose pane is gone or has exited.

use moor_core::activity::PaneStatus;
use moor_core::config::{Flags, TaskSettings};
use moor_core::decide::transcript::Trigger;
use moor_core::domain::decision::{Decision, ExtraEvent, Field, Fields};
use moor_core::domain::task::TaskState;
use moor_core::seam::proc::Deadline;
use moor_core::seam::surface::{PaneRef, SurfaceError};
use moor_core::view::AgentLaunchView;
use moor_core::{ErrorKind, StartReq, TaskId, Warning};
use std::path::Path;
use std::time::Duration;

use super::launch;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::boxlock::{self, BoxLock};
use crate::ops::common::facts::settings;
use crate::ops::common::provision::reread;
use crate::ops::common::resume;
use crate::ops::common::transcript;
use crate::store::{SessionRow, Store, TaskRow};

/// Time allowed for reading a pane to see whether it ended.
const PANE_DEADLINE: Duration = Duration::from_secs(5);

/// How a session's pane ended.
enum Ended {
    /// The pane no longer exists.
    Gone,
    /// The agent left its pane showing the exit marker.
    Exited(i32),
}

/// Launches, or relaunches, the agent of an attached Task.
///
/// The Box lock is held from the live-session check through the launch, so two starts cannot both
/// launch; it is released on return, before anything waits on the agent.
///
/// # Errors
/// A store failure, a busy Box, a model that cannot be resolved, or a `.moor.yaml` that changed
/// since the base commit when an agent is about to launch and the owner has not trusted it.
///
/// `setup_exit` is the setup exit of a Box just provisioned, and whether that Box replaced a lost one.
///
/// `settings` is evaluated once, before anything changes, and holds the config the launch uses.
pub(super) fn run(
    cx: &Cx,
    store: &Store,
    req: &StartReq,
    id: TaskId,
    (setup_exit, recreated): (Option<i32>, bool),
    settings: &dyn Fn() -> Result<TaskSettings, CommandError>,
) -> Result<AgentLaunchView, CommandError> {
    let row = reread(store, id)?;
    if req.no_agent || row.state != TaskState::Working {
        return launch::run(cx, store, req, id, setup_exit, settings);
    }
    match store.live_session(id)? {
        Some(s) if ended(cx, &s).is_none() => {
            on_live(cx, store, req, &row, &s);
            return Ok(AgentLaunchView::default());
        }
        None if row.goal.is_none() && req.goal.is_none() && !req.resume => {
            return Ok(AgentLaunchView::default())
        }
        _ => {}
    }
    let _held = BoxLock::acquire(cx.clock(), cx.state_dir()?, id, "start", boxlock::WAIT)?;
    let Some(old) = store.live_session(id)? else {
        if req.goal.is_some() || row.goal.is_some() || resumes(req, &row) {
            let cfg = settings()?;
            store_goal(store, req, id)?;
            return launch::run(cx, store, req, id, setup_exit, &|| Ok(cfg.clone()));
        }
        store_goal(store, req, id)?;
        return launch::run(cx, store, req, id, setup_exit, settings);
    };
    let Some(end) = ended(cx, &old) else {
        on_live(cx, store, req, &row, &old);
        return Ok(AgentLaunchView::default());
    };
    relaunch(cx, store, req, (&old, &end), recreated, settings, None)
}

/// Whether [`run`] is about to launch or relaunch the agent of `row`.
///
/// # Errors
/// A store failure.
pub(super) fn will_launch(
    cx: &Cx,
    store: &Store,
    req: &StartReq,
    row: &TaskRow,
) -> Result<bool, CommandError> {
    if req.no_agent {
        return Ok(false);
    }
    if !matches!(row.state, TaskState::Working | TaskState::Idle) {
        return Ok(true);
    }
    Ok(store.live_session(row.id)?.map_or_else(
        || row.goal.is_some() || req.goal.is_some() || req.resume,
        |s| ended(cx, &s).is_some(),
    ))
}

/// Whether an agent session of `row` exists and has not ended.
pub(super) fn agent_live(cx: &Cx, store: &Store, row: &TaskRow) -> Result<bool, CommandError> {
    Ok(store
        .live_session(row.id)?
        .is_some_and(|s| ended(cx, &s).is_none()))
}

/// Whether `req` resumes the stored agent session of `row`.
fn resumes(req: &StartReq, row: &TaskRow) -> bool {
    req.resume
        && req.goal.is_none()
        && row
            .agent_session_id
            .as_deref()
            .is_some_and(|s| !s.is_empty())
}

/// Refuses `--resume` for a Task with no stored agent session.
///
/// # Errors
/// [`ErrorKind::Refused`] when `req` resumes without a goal and `row` holds no session id.
pub(super) fn check_resume(req: &StartReq, row: &TaskRow) -> Result<(), CommandError> {
    let stored = row
        .agent_session_id
        .as_deref()
        .is_some_and(|s| !s.is_empty());
    if req.resume && req.goal.is_none() && !stored {
        return Err(nothing_to_resume());
    }
    Ok(())
}

/// The refusal for `--resume` with no agent session to resume.
pub(super) fn nothing_to_resume() -> CommandError {
    CommandError::new(ErrorKind::Refused, "no agent session to resume")
}

/// Handles a start that finds the agent live: warns of a goal it cannot take, and with
/// `--resume` registers the session with the multiplexer again.
fn on_live(cx: &Cx, store: &Store, req: &StartReq, row: &TaskRow, session: &SessionRow) {
    warn_goal_ignored(cx, req, &row.name);
    let (true, Some(pane)) = (req.resume, session.pane.as_deref()) else {
        return;
    };
    let state = cx
        .surface()
        .pane_obs(&PaneRef::new(pane), Deadline(PANE_DEADLINE))
        .map_or(PaneStatus::Unknown, |obs| obs.status);
    if let Some(agent) = row.agent_session_id.as_deref().filter(|a| !a.is_empty()) {
        if resume::report(cx, &row.name, pane, agent, state) {
            if let Err(e) = store.set_registered_session(session.id, agent) {
                cx.warn(Warning {
                    code: "resume_record_failed".to_owned(),
                    message: format!("could not record the resume registration: {e}"),
                });
            }
        }
    }
}

/// Stores a given goal on the Task, replacing any it holds, so the launch brief carries it.
fn store_goal(store: &Store, req: &StartReq, id: TaskId) -> Result<(), CommandError> {
    let Some(g) = &req.goal else {
        return Ok(());
    };
    let set = Fields {
        goal: Field::Set(g.clone()),
        ..Fields::default()
    };
    store.apply(Decision::new(id).set(set))?;
    Ok(())
}

/// Replaces the ended session `old` with a new one.
///
/// A given goal replaces the Task's goal and starts a fresh brief. Without one the agent resumes
/// its stored session, else starts a fresh brief with the Task's goal. The old tab stays.
fn relaunch(
    cx: &Cx,
    store: &Store,
    req: &StartReq,
    (old, end): (&SessionRow, &Ended),
    recreated: bool,
    settings: &dyn Fn() -> Result<TaskSettings, CommandError>,
    cause: Option<&str>,
) -> Result<AgentLaunchView, CommandError> {
    let id = old.task;
    let mut row = reread(store, id)?;
    let resume = match &req.goal {
        Some(_) => None,
        None => row.agent_session_id.clone().filter(|s| !s.is_empty()),
    };
    let goal = req.goal.clone().or_else(|| row.goal.clone());
    if goal.is_none() && resume.is_none() {
        return Ok(AgentLaunchView::default());
    }
    row.goal.clone_from(&goal);
    let Some(mut launch) = launch::launch_req(cx, &row, None) else {
        return Ok(AgentLaunchView::default());
    };
    let cfg = settings()?;
    launch.resume = resume;
    launch.model = cfg.model.clone().map(|m| m.value);
    if let Some(g) = &req.goal {
        let set = Fields {
            goal: Field::Set(g.clone()),
            ..Fields::default()
        };
        store.apply(Decision::new(id).set(set))?;
    }
    let exit = match end {
        Ended::Gone => None,
        Ended::Exited(code) => Some(*code),
    };
    // A resumed session keeps writing the same file on a disk that survived, so a copy now adds nothing.
    let saved = if launch.resume.is_some() && !recreated {
        transcript::record_skipped(store, &row, Trigger::Relaunch, "resumed on the same Box");
        None
    } else {
        transcript::save(cx, store, &row, old, Trigger::Relaunch)
    };
    let digest = saved.as_ref().map(|t| t.digest.as_str());
    store.end_session(old.id, exit, digest)?;
    let view = launch::begin(cx, store, &row, &launch, &cfg)?;
    if view.launched {
        let new_pane = store.live_session(id)?.and_then(|s| s.pane);
        let event = ExtraEvent::Relaunched {
            old_pane: old.pane.clone(),
            new_pane,
            goal,
            cause: cause.map(str::to_owned),
        };
        store.apply(Decision::new(id).event(event))?;
    }
    Ok(view)
}

/// Starts the agent again after a Box wake when its pane did not survive.
///
/// The caller holds the Box lock. A live agent, or a Task with no live session or not Working, is
/// left alone. The stored agent session is resumed; a Task with neither one nor a goal stays as is.
///
/// # Errors
/// A store failure, or settings that cannot be resolved.
pub(crate) fn revive_after_wake(cx: &Cx, store: &Store, id: TaskId) -> Result<(), CommandError> {
    let row = reread(store, id)?;
    if row.state != TaskState::Working {
        return Ok(());
    }
    let Some(old) = store.live_session(id)? else {
        return Ok(());
    };
    let Some(end) = ended(cx, &old) else {
        return Ok(());
    };
    let req = StartReq {
        name: row.name.clone(),
        goal: None,
        issue: None,
        worktree: None,
        base: None,
        model: None,
        no_agent: false,
        resume: false,
        trust_moor_yaml: false,
    };
    let settings = || {
        let at = row.worktree.as_deref().unwrap_or(&row.repo);
        settings(cx, Path::new(at), &Flags::default())
    };
    relaunch(cx, store, &req, (&old, &end), false, &settings, Some("wake"))?;
    Ok(())
}

/// How the pane of `session` ended; none while it is live, or when it cannot be read.
fn ended(cx: &Cx, session: &SessionRow) -> Option<Ended> {
    let pane = PaneRef::new(session.pane.as_deref()?);
    match cx.surface().pane_obs(&pane, Deadline(PANE_DEADLINE)) {
        Ok(obs) => obs.exit_marker.map(Ended::Exited),
        Err(SurfaceError::NotFound) => Some(Ended::Gone),
        Err(_) => None,
    }
}

/// Warns that a given goal does not reach a live agent.
fn warn_goal_ignored(cx: &Cx, req: &StartReq, task: &moor_core::TaskName) {
    if req.goal.is_some() {
        cx.warn(Warning {
            code: "goal_ignored".to_owned(),
            message: format!(
                "--goal ignored: the agent for {} is live; use moor steer",
                task.as_str()
            ),
        });
    }
}
