//! Starts the agent again when a wake or a steer finds it gone.

use std::time::Duration;

use moor_core::config::Flags;
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::seam::proxy_host::ProxyStatus;
use moor_core::view::AgentLaunchView;
use moor_core::{BoxId, ErrorDetail, ErrorKind, StartReq};

use super::relaunch::{ended, record_relaunch, relaunch, Ended};
use super::{launch, reopen, watch};
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::facts::settings_of;
use crate::ops::common::provision::reread;
use crate::store::{BoxRow, SessionRow, Store};

/// Longest a wake waits for the tunnel before relaunching anyway.
const TUNNEL_WAIT: Duration = Duration::from_secs(60);
/// How often the tunnel is checked while waiting.
const TUNNEL_POLL: Duration = Duration::from_secs(1);
/// How long the tunnel must go without restarting before the agent relaunches.
const TUNNEL_SETTLE: Duration = Duration::from_secs(5);
/// Automatic relaunches one Box may have within `RELAUNCH_WINDOW`.
const RELAUNCH_CAP: usize = 3;

/// The window `RELAUNCH_CAP` counts over.
const RELAUNCH_WINDOW: Duration = Duration::from_mins(10);

/// Recent events searched for relaunches.
const CAP_EVENTS: usize = 100;

/// Recent events searched for a tunnel restart.
const TUNNEL_EVENTS: usize = 50;

/// Starts the agent again after a Box wake when its pane did not survive.
///
/// The caller holds the Box lock. A live agent, or a Box with no live session or not Working, is
/// left alone. The stored agent session is resumed, else the stored prompt starts a fresh agent; a
/// Box with neither stays as is.
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
    revive(cx, store, &row, ended_pane.as_ref(), "wake").map(drop)
}

/// Starts the agent again, resuming its stored agent session, for a steer that found it exited.
///
/// The caller holds the Box lock and saw the agent gone, so an unreadable pane counts as gone. A
/// Box that is not Ready, or has nothing to resume, launches nothing.
///
/// # Errors
/// A store failure, or settings that cannot be resolved.
pub(crate) fn revive_for_steer(
    cx: &Cx,
    store: &Store,
    id: BoxId,
) -> Result<AgentLaunchView, CommandError> {
    let row = reread(store, id)?;
    if row.lifecycle != BoxLifecycle::Ready {
        return Ok(AgentLaunchView::default());
    }
    let ended_pane = store.live_session(id)?.map(|s| {
        let end = ended(cx, &s).unwrap_or(Ended::Gone);
        (s, end)
    });
    revive(cx, store, &row, ended_pane.as_ref(), "steer")
}

/// Relaunches the agent of `row` whose pane ended, or launches into a new Session when the last
/// one ended, and watches the launch. Only a steer resumes the stored session in a new Session.
fn revive(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    ended_pane: Option<&(SessionRow, Ended)>,
    cause: &str,
) -> Result<AgentLaunchView, CommandError> {
    let id = row.id;
    if !revivable(row) || (ended_pane.is_none() && !reopen::ended_session(store, id, None)?) {
        return Ok(AgentLaunchView::default());
    }
    check_cap(cx, store, row)?;
    super::wake_idle(store, id)?;
    await_tunnel(cx, store, id);
    let resumable = row
        .agent_session_id
        .as_deref()
        .is_some_and(|s| !s.is_empty());
    let req = StartReq {
        name: row.name.clone(),
        prompt: (!resumable).then(|| row.prompt.clone()).flatten(),
        issue: None,
        base: None,
        model: None,
        provider: None,
        no_agent: false,
        resume: cause == "steer",
        repo: None,
        parent: None,
        identity: None,
    };
    let settings = || settings_of(cx, row, &Flags::default());
    let view = match ended_pane {
        None => {
            let view = launch::run(cx, store, &req, id, None, &settings)?;
            if view.launched && cause == "steer" {
                record_relaunch(store, id, None, row.prompt.clone(), Some(cause))?;
            }
            view
        }
        Some((old, end)) => relaunch(cx, store, &req, (old, end), (false, Some(cause)), &settings)?,
    };
    Ok(watch::run(cx, store, id, view))
}

/// Refuses another automatic relaunch once `RELAUNCH_CAP` have happened within `RELAUNCH_WINDOW`.
///
/// Only the first refusal after a relaunch is recorded as `relaunch_capped`. An explicit
/// `moor start --resume` is not an automatic relaunch and never meets the cap.
fn check_cap(cx: &Cx, store: &Store, row: &BoxRow) -> Result<(), CommandError> {
    let now = cx.clock().now().as_millis();
    let window = i64::try_from(RELAUNCH_WINDOW.as_millis()).unwrap_or(i64::MAX);
    let events = store.events(row.id, CAP_EVENTS)?;
    let recent = events
        .iter()
        .filter(|e| {
            e.kind == "relaunched"
                && e.payload.get("cause").is_some_and(|c| !c.is_null())
                && now.saturating_sub(e.ts.as_millis()) < window
        })
        .count();
    if recent < RELAUNCH_CAP {
        return Ok(());
    }
    let recorded = events
        .iter()
        .rev()
        .find(|e| matches!(e.kind.as_str(), "relaunched" | "relaunch_capped"))
        .is_some_and(|e| e.kind == "relaunch_capped");
    if !recorded {
        let event = ExtraEvent::RelaunchCapped {
            count: recent,
            window_secs: RELAUNCH_WINDOW.as_secs(),
        };
        store.apply(Decision::new(row.id).event(event))?;
    }
    let name = row.name.as_str();
    Err(CommandError::new(
        ErrorKind::Refused,
        format!(
            "moor relaunched the agent of Box {name} {recent} times in the last {} minutes, so it will not relaunch it again on its own; once you know why it keeps ending, start it again with `moor start {name} --resume`",
            RELAUNCH_WINDOW.as_secs() / 60
        ),
    )
    .with_detail(ErrorDetail::RelaunchCapped))
}

/// Whether `row` holds a prompt or an agent session to start again.
fn revivable(row: &BoxRow) -> bool {
    row.prompt.is_some()
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
