//! Starts the agent again for a Working Box whose pane is gone or has exited.

use moor_core::activity::PaneStatus;
use moor_core::config::{BoxSettings, Flags};
use moor_core::decide::transcript::Trigger;
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::decision::{Decision, ExtraEvent, Field, Fields};
use moor_core::seam::engine::LaunchPrompt;
use moor_core::seam::proc::Deadline;
use moor_core::seam::proxy_host::ProxyStatus;
use moor_core::seam::surface::{PaneRef, SurfaceError};
use moor_core::view::AgentLaunchView;
use moor_core::{BoxId, ErrorKind, SessionId, StartReq, Warning};
use std::path::Path;
use std::time::Duration;

use super::launch;
use super::reopen;
use super::watch;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::boxlock::{self, BoxLock};
use crate::ops::common::facts::settings;
use crate::ops::common::provision::reread;
use crate::ops::common::resume;
use crate::ops::common::transcript;
use crate::store::{BoxRow, SessionRow, Store};

/// Time allowed for reading a pane to see whether it ended.
const PANE_DEADLINE: Duration = Duration::from_secs(5);

/// Longest a wake waits for the tunnel before relaunching anyway.
const TUNNEL_WAIT: Duration = Duration::from_secs(60);
/// How often the tunnel is checked while waiting.
const TUNNEL_POLL: Duration = Duration::from_secs(1);
/// How long the tunnel must go without restarting before the agent relaunches.
const TUNNEL_SETTLE: Duration = Duration::from_secs(5);
/// Recent events searched for a tunnel restart.
const TUNNEL_EVENTS: usize = 50;

/// How a session's pane ended.
enum Ended {
    /// The pane no longer exists.
    Gone,
    /// The agent left its pane showing the exit marker.
    Exited(i32),
}

/// Launches, or relaunches, the agent of an attached Box.
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
    id: BoxId,
    (setup_exit, recreated): (Option<i32>, bool),
    settings: &dyn Fn() -> Result<BoxSettings, CommandError>,
) -> Result<AgentLaunchView, CommandError> {
    let row = reread(store, id)?;
    if req.no_agent || !launch::working(&row) {
        return launch::run(cx, store, req, id, setup_exit, settings);
    }
    match store.live_session(id)? {
        Some(s) if ended(cx, &s).is_none() => {
            on_live(cx, store, req, &row, &s);
            return Ok(AgentLaunchView::default());
        }
        None if row.goal.is_none() && req.goal.is_none() && !req.resume => {
            return Ok(AgentLaunchView::default());
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
    relaunch(cx, store, req, (&old, &end), (recreated, None), settings)
}

/// Whether `req` resumes the stored agent session of `row`.
fn resumes(req: &StartReq, row: &BoxRow) -> bool {
    req.resume
        && req.goal.is_none()
        && row
            .agent_session_id
            .as_deref()
            .is_some_and(|s| !s.is_empty())
}

/// Refuses `--resume` for a Box with no stored agent session.
///
/// # Errors
/// [`ErrorKind::Refused`] when `req` resumes without a goal and `row` holds no session id.
pub(super) fn check_resume(req: &StartReq, row: &BoxRow) -> Result<(), CommandError> {
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
fn on_live(cx: &Cx, store: &Store, req: &StartReq, row: &BoxRow, session: &SessionRow) {
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

/// Stores a given goal on the Box, replacing any it holds, so the launch brief carries it.
fn store_goal(store: &Store, req: &StartReq, id: BoxId) -> Result<(), CommandError> {
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
/// A given goal replaces the Box's goal and starts a fresh brief. Without one the agent resumes
/// its stored session, else starts a fresh brief with the Box's goal. The old tab stays.
fn relaunch(
    cx: &Cx,
    store: &Store,
    req: &StartReq,
    (old, end): (&SessionRow, &Ended),
    (recreated, cause): (bool, Option<&str>),
    settings: &dyn Fn() -> Result<BoxSettings, CommandError>,
) -> Result<AgentLaunchView, CommandError> {
    let id = old.box_id;
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
    if req.hold {
        launch.prompt = LaunchPrompt::None;
    }
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
    store.end_current_session(id, "relaunch", exit)?;
    let next = store.open_child_session(id, req.parent.map(SessionId::get))?;
    store.start_session(next.id)?;
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
/// The caller holds the Box lock. A live agent, or a Box with no live session or not Working, is
/// left alone. The stored agent session is resumed; a Box with neither one nor a goal stays as is.
/// The new pane is watched as `start` watches it, so an agent that dies at launch, such as one
/// stopped because the Proxy CA could not be written, is recorded as a launch failure.
///
/// # Errors
/// A store failure, or settings that cannot be resolved.
pub(crate) fn revive_after_wake(cx: &Cx, store: &Store, id: BoxId) -> Result<(), CommandError> {
    let row = reread(store, id)?;
    if row.lifecycle != BoxLifecycle::Ready {
        return Ok(());
    }
    let ended_pane = match store.live_session(id)? {
        Some(s) if launch::working(&row) => match ended(cx, &s) {
            Some(end) => Some((s, end)),
            None => return Ok(()),
        },
        Some(_) => return Ok(()),
        None => None,
    };
    if ended_pane.is_none() && !(revivable(&row) && reopen::ended_session(store, id, None)?) {
        return Ok(());
    }
    super::wake_idle(store, id)?;
    await_tunnel(cx, store, id);
    let req = StartReq {
        name: row.name.clone(),
        goal: None,
        issue: None,
        base: None,
        model: None,
        no_agent: false,
        resume: false,
        trust_moor_yaml: false,
        repo: None,
        hold: false,
        parent: None,
        identity: None,
    };
    let settings = || {
        let at = row.worktree.as_deref().unwrap_or(&row.repo);
        settings(cx, Path::new(at), &Flags::default())
    };
    let view = match &ended_pane {
        None => launch::run(cx, store, &req, id, None, &settings)?,
        Some((old, end)) => relaunch(
            cx,
            store,
            &req,
            (old, end),
            (false, Some("wake")),
            &settings,
        )?,
    };
    watch::run(cx, store, id, view);
    Ok(())
}

/// Whether `row` holds a goal or an agent session to start again.
fn revivable(row: &BoxRow) -> bool {
    row.goal.is_some()
        || row
            .agent_session_id
            .as_deref()
            .is_some_and(|s| !s.is_empty())
}

/// Waits until the Box's tunnel is up and has not restarted for `TUNNEL_SETTLE`.
///
/// A relaunch touches the Box lease, which every tunnel restart rewrites, so launching into a
/// restarting tunnel loses the claim. A Box with no live proxy is not waited for. After
/// `TUNNEL_WAIT` the relaunch goes ahead anyway.
fn await_tunnel(cx: &Cx, store: &Store, id: BoxId) {
    let mut waited = Duration::ZERO;
    loop {
        let up = match cx.proxy_host().status(id) {
            Ok(ProxyStatus::Ready { .. }) => true,
            Ok(ProxyStatus::Starting) => false,
            _ => return,
        };
        if up && !restarted_within(cx, store, id, TUNNEL_SETTLE) {
            return;
        }
        if waited >= TUNNEL_WAIT {
            return;
        }
        cx.clock().sleep(TUNNEL_POLL);
        waited += TUNNEL_POLL;
    }
}

/// Whether the Box's tunnel restarted less than `window` ago.
fn restarted_within(cx: &Cx, store: &Store, id: BoxId, window: Duration) -> bool {
    let now = cx.clock().now().as_millis();
    let window = i64::try_from(window.as_millis()).unwrap_or(i64::MAX);
    store.events(id, TUNNEL_EVENTS).is_ok_and(|events| {
        events
            .iter()
            .rev()
            .find(|e| e.kind == "tunnel_restarted")
            .is_some_and(|e| now.saturating_sub(e.ts.as_millis()) < window)
    })
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
fn warn_goal_ignored(cx: &Cx, req: &StartReq, name: &moor_core::BoxName) {
    if req.goal.is_some() {
        cx.warn(Warning {
            code: "goal_ignored".to_owned(),
            message: format!(
                "--goal ignored: the agent for {} is live; use moor steer",
                name.as_str()
            ),
        });
    }
}
