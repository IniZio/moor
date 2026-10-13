//! Starts the agent again for a Working Task whose pane is gone or has exited.

use moor_core::decide::transcript::Trigger;
use moor_core::domain::decision::{Decision, ExtraEvent, Field, Fields};
use moor_core::domain::task::TaskState;
use moor_core::seam::proc::Deadline;
use moor_core::seam::surface::{PaneRef, SurfaceError};
use moor_core::view::AgentLaunchView;
use moor_core::{StartReq, TaskId, Warning};
use std::time::Duration;

use super::launch;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::boxlock::{self, BoxLock};
use crate::ops::common::provision::reread;
use crate::ops::common::transcript;
use crate::store::{SessionRow, Store};

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
/// A store failure, a busy Box, or a model that cannot be resolved.
pub(super) fn run(
    cx: &Cx,
    store: &Store,
    req: &StartReq,
    id: TaskId,
    setup_exit: Option<i32>,
    model: &dyn Fn() -> Result<Option<String>, CommandError>,
) -> Result<AgentLaunchView, CommandError> {
    let row = reread(store, id)?;
    if req.no_agent || row.state != TaskState::Working {
        return launch::run(cx, store, req, id, setup_exit, model);
    }
    match store.live_session(id)? {
        Some(s) if ended(cx, &s).is_none() => {
            warn_goal_ignored(cx, req, &row.name);
            return Ok(AgentLaunchView::default());
        }
        None if row.goal.is_none() && req.goal.is_none() => return Ok(AgentLaunchView::default()),
        _ => {}
    }
    let _held = BoxLock::acquire(cx.clock(), cx.state_dir()?, id, "start", boxlock::WAIT)?;
    let Some(old) = store.live_session(id)? else {
        store_goal(store, req, id)?;
        return launch::run(cx, store, req, id, setup_exit, model);
    };
    let Some(end) = ended(cx, &old) else {
        warn_goal_ignored(cx, req, &row.name);
        return Ok(AgentLaunchView::default());
    };
    relaunch(cx, store, req, &old, &end, model)
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
    old: &SessionRow,
    end: &Ended,
    model: &dyn Fn() -> Result<Option<String>, CommandError>,
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
    launch.resume = resume;
    launch.model = model()?;
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
    let saved = transcript::save(cx, store, &row, old, Trigger::Relaunch);
    let digest = saved.as_ref().map(|t| t.digest.as_str());
    store.end_session(old.id, exit, digest)?;
    let view = launch::begin(cx, store, &row, &launch)?;
    if view.launched {
        let new_pane = store.live_session(id)?.and_then(|s| s.pane);
        let event = ExtraEvent::Relaunched {
            old_pane: old.pane.clone(),
            new_pane,
            goal,
        };
        store.apply(Decision::new(id).event(event))?;
    }
    Ok(view)
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
