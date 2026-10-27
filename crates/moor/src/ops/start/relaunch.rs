//! Starts the agent again for a Working Box whose pane is gone or has exited.

use moor_core::activity::PaneStatus;
use moor_core::config::BoxSettings;
use moor_core::decide::transcript::Trigger;
use moor_core::domain::decision::{Decision, ExtraEvent, Field, Fields};
use moor_core::seam::engine::LaunchPrompt;
use moor_core::seam::proc::Deadline;
use moor_core::seam::surface::{PaneRef, SurfaceError};
use moor_core::view::AgentLaunchView;
use moor_core::{BoxId, ErrorKind, SessionId, StartReq, Warning};
use std::time::Duration;

use super::launch;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::boxlock::{self, BoxLock};
use crate::ops::common::provision::reread;
use crate::ops::common::resume;
use crate::ops::common::transcript;
use crate::store::{BoxRow, SessionRow, Store};

/// Time allowed for reading a pane to see whether it ended.
const PANE_DEADLINE: Duration = Duration::from_secs(5);

/// How a session's pane ended.
pub(super) enum Ended {
    /// The pane no longer exists.
    Gone,
    /// The agent left its pane showing the exit marker.
    Exited(i32),
}

/// Launches, or relaunches, the agent of an attached Box unless `req` gives `--no-agent`.
///
/// A start without a prompt never sends the stored prompt again on its own.
///
/// # Errors
/// As [`launch_asked`].
pub(super) fn run(
    cx: &Cx,
    store: &Store,
    req: &StartReq,
    id: BoxId,
    setup: (Option<i32>, bool),
    settings: &dyn Fn() -> Result<BoxSettings, CommandError>,
) -> Result<AgentLaunchView, CommandError> {
    if !asks_for_agent(req) {
        return Ok(AgentLaunchView::default());
    }
    launch_asked(cx, store, req, id, setup, settings)
}

/// Launches the agent of an attached Box, or relaunches it once its pane has ended.
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
fn launch_asked(
    cx: &Cx,
    store: &Store,
    req: &StartReq,
    id: BoxId,
    (setup_exit, recreated): (Option<i32>, bool),
    settings: &dyn Fn() -> Result<BoxSettings, CommandError>,
) -> Result<AgentLaunchView, CommandError> {
    let row = reread(store, id)?;
    if !launch::working(&row) {
        return launch::run(cx, store, req, id, setup_exit, settings);
    }
    if let Some(s) = store.live_session(id)?.filter(|s| ended(cx, s).is_none()) {
        on_live(cx, store, req, &row, &s);
        return Ok(AgentLaunchView::default());
    }
    let _held = BoxLock::acquire(cx.clock(), cx.state_dir()?, id, "start", boxlock::WAIT)?;
    let Some(old) = store.live_session(id)? else {
        let cfg = settings()?;
        store_prompt(store, req, id)?;
        return launch::run(cx, store, req, id, setup_exit, &|| Ok(cfg.clone()));
    };
    let Some(end) = ended(cx, &old) else {
        on_live(cx, store, req, &row, &old);
        return Ok(AgentLaunchView::default());
    };
    relaunch(cx, store, req, (&old, &end), (recreated, None), settings)
}

/// Whether a start of an open Box asks for its agent: any start without `--no-agent`.
pub(super) const fn asks_for_agent(req: &StartReq) -> bool {
    !req.no_agent
}

/// Refuses `--resume` for a Box with no stored agent session.
///
/// # Errors
/// [`ErrorKind::Refused`] when `req` resumes without a prompt and `row` holds no session id.
pub(super) fn check_resume(req: &StartReq, row: &BoxRow) -> Result<(), CommandError> {
    let stored = row
        .agent_session_id
        .as_deref()
        .is_some_and(|s| !s.is_empty());
    if req.resume && req.prompt.is_none() && !stored {
        return Err(nothing_to_resume());
    }
    Ok(())
}

/// The refusal for `--resume` with no agent session to resume.
pub(super) fn nothing_to_resume() -> CommandError {
    CommandError::new(ErrorKind::Refused, "no agent session to resume")
}

/// Handles a start that finds the agent live: warns of a prompt it cannot take, and with
/// `--resume` registers the session with the multiplexer again.
fn on_live(cx: &Cx, store: &Store, req: &StartReq, row: &BoxRow, session: &SessionRow) {
    warn_prompt_ignored(cx, req, &row.name);
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

/// Stores a given prompt on the Box, replacing any it holds, so the launch sends it.
fn store_prompt(store: &Store, req: &StartReq, id: BoxId) -> Result<(), CommandError> {
    let Some(g) = &req.prompt else {
        return Ok(());
    };
    let set = Fields {
        prompt: Field::Set(g.clone()),
        ..Fields::default()
    };
    store.apply(Decision::new(id).set(set))?;
    Ok(())
}

/// Replaces the ended session `old` with a new one.
///
/// A given prompt replaces the Box's prompt and starts a fresh agent on it. Without one the agent
/// resumes its stored session, else starts fresh waiting for a prompt. The old tab stays.
pub(super) fn relaunch(
    cx: &Cx,
    store: &Store,
    req: &StartReq,
    (old, end): (&SessionRow, &Ended),
    (recreated, cause): (bool, Option<&str>),
    settings: &dyn Fn() -> Result<BoxSettings, CommandError>,
) -> Result<AgentLaunchView, CommandError> {
    let id = old.box_id;
    let row = reread(store, id)?;
    let resume = match &req.prompt {
        Some(_) => None,
        None => row.agent_session_id.clone().filter(|s| !s.is_empty()),
    };
    let Some(mut launch) = launch::launch_req(cx, &row, None) else {
        return Ok(AgentLaunchView::default());
    };
    let cfg = settings()?;
    launch.resume = resume;
    launch.prompt = req
        .prompt
        .clone()
        .map_or(LaunchPrompt::None, LaunchPrompt::Text);
    launch.model = cfg.model.clone().map(|m| m.value);
    if let Some(g) = &req.prompt {
        let set = Fields {
            prompt: Field::Set(g.clone()),
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
    store.end_current_session(id, "relaunch", exit)?;
    let next = store.open_child_session(id, req.parent.map(SessionId::get))?;
    store.start_session(next.id)?;
    let view = launch::begin(cx, store, &row, &launch, &cfg)?;
    if view.launched {
        record_relaunch(store, id, old.pane.clone(), req.prompt.clone(), cause)?;
    }
    Ok(view)
}

/// Appends the `relaunched` event of the agent now live in Box `id`.
pub(super) fn record_relaunch(
    store: &Store,
    id: BoxId,
    old_pane: Option<String>,
    prompt: Option<String>,
    cause: Option<&str>,
) -> Result<(), CommandError> {
    let new_pane = store.live_session(id)?.and_then(|s| s.pane);
    let cause = cause.map(str::to_owned);
    let event = ExtraEvent::Relaunched {
        old_pane,
        new_pane,
        prompt,
        cause,
    };
    store.apply(Decision::new(id).event(event))?;
    Ok(())
}

/// How the pane of `session` ended; none while it is live, or when it cannot be read.
pub(super) fn ended(cx: &Cx, session: &SessionRow) -> Option<Ended> {
    let pane = PaneRef::new(session.pane.as_deref()?);
    match cx.surface().pane_obs(&pane, Deadline(PANE_DEADLINE)) {
        Ok(obs) => obs.exit_marker.map(Ended::Exited),
        Err(SurfaceError::NotFound) => Some(Ended::Gone),
        Err(_) => None,
    }
}

/// Warns that a given prompt does not reach a live agent.
fn warn_prompt_ignored(cx: &Cx, req: &StartReq, name: &moor_core::BoxName) {
    if req.prompt.is_some() {
        cx.warn(Warning {
            code: "prompt_ignored".to_owned(),
            message: format!(
                "--prompt ignored: the agent for {} is live; use moor steer",
                name.as_str()
            ),
        });
    }
}
