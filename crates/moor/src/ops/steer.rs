//! `steer`: send a message or a menu choice to the Task's agent.
//!
//! The Box lock is taken only to wake an asleep Task, so a Task can be steered during a run.

use std::time::Duration;

use moor_core::activity::AgentState;
use moor_core::decide::transcript::Trigger;
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::boxes::BoxState;
use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::seam::engine::{EngineError, SessionRef, Steer, SteerOutcome};
use moor_core::seam::proc::Deadline;
use moor_core::view::steer::SteerValue;
use moor_core::{BoxId, ErrorKind, SteerReq, Warning};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::activity::{self, session_ref};
use crate::ops::common::boxlock::{self, BoxLock};
use crate::ops::common::transcript;
use crate::ops::common::wake::wake_and_revive;
use crate::store::{BoxRow, SessionRow, Store};

/// Longest the send may take, pane reads included.
const STEER_DEADLINE: Duration = Duration::from_secs(30);

/// Run `steer`.
///
/// # Errors
/// [`CommandError`] when the Task is unknown or has no live agent, the agent ended, the choice
/// does not fit what the agent shows, or the send failed.
pub(crate) fn run(cx: &Cx, req: &SteerReq) -> Result<SteerValue, CommandError> {
    let store = cx.store()?;
    let name = req.box_name.as_str();
    let id = store
        .open_task_by_name(&req.box_name)?
        .ok_or_else(|| CommandError::new(ErrorKind::Failed, format!("no open task {name}")))?
        .id;
    let no_agent = || {
        CommandError::new(
            ErrorKind::Refused,
            format!(
                "no live agent session for task {name}; start it with {}",
                relaunch(name)
            ),
        )
    };
    let reload = || {
        store
            .task(id)?
            .ok_or_else(|| CommandError::new(ErrorKind::Failed, format!("no task {name}")))
    };
    if store.live_session(id)?.is_none() {
        return Err(no_agent());
    }
    if asleep(&reload()?) {
        let _held = BoxLock::acquire(cx.clock(), cx.state_dir()?, id, "steer", boxlock::WAIT)?;
        wake_and_revive(cx, store, &reload()?)?;
    }
    let row = reload()?;
    let session = store.live_session(id)?.ok_or_else(no_agent)?;
    let sref = session_ref(&row, session.session_ref.clone(), session.pane.clone())
        .ok_or_else(no_agent)?;
    let before = state_of(cx, store, &row)?;
    let steer = choose(req, before.as_ref())?;
    let sent = match &steer {
        Steer::Text(t) => t.clone(),
        Steer::Menu(n) => n.to_string(),
    };
    let Some(outcome) = send(
        cx,
        store,
        &row,
        (name, &session, &sref),
        &steer,
        before.as_ref(),
    )?
    else {
        return exited(cx, store, (id, name), &session, sent);
    };
    let steer_id = record(store, id, &session, &sent, outcome.submit_retried)?;
    let answered_menu = answered(cx, store, &row, &steer, outcome)?;
    Ok(SteerValue {
        task: name.to_owned(),
        sent,
        answered_menu,
        steer: steer_id,
    })
}

/// The result of an exit steer whose pane was already gone: recorded, with a warning.
fn exited(
    cx: &Cx,
    store: &Store,
    (id, name): (BoxId, &str),
    session: &SessionRow,
    sent: String,
) -> Result<SteerValue, CommandError> {
    let steer = record(store, id, session, &sent, false)?;
    cx.warn(Warning {
        code: "agent_exited".to_owned(),
        message: "the agent exited and its session ended".to_owned(),
    });
    Ok(SteerValue {
        task: name.to_owned(),
        sent,
        answered_menu: false,
        steer,
    })
}

/// Whether the Task or its Box is asleep.
fn asleep(row: &BoxRow) -> bool {
    row.lifecycle == BoxLifecycle::Asleep
        || row
            .boxed
            .as_ref()
            .is_some_and(|b| b.state == BoxState::Asleep)
}

/// The command that starts the agent again.
fn relaunch(name: &str) -> String {
    format!("`moor start {name} --goal \"...\"`")
}

/// The agent's state now; none when the Task has no agent to read.
fn state_of(cx: &Cx, store: &Store, row: &BoxRow) -> Result<Option<AgentState>, CommandError> {
    let reading = activity::read(cx, store, row, cx.clock().now())?;
    Ok(reading.activity.map(|a| a.state))
}

/// Whether `steer` asks the agent to exit.
fn is_exit(steer: &Steer) -> bool {
    matches!(steer, Steer::Text(t) if t.trim() == "/exit")
}

/// Sends `steer`; a pane that ended has its transcript saved and its session stopped, then an
/// exit steer yields `None` and any other steer refuses.
fn send(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    (name, session, sref): (&str, &SessionRow, &SessionRef),
    steer: &Steer,
    before: Option<&AgentState>,
) -> Result<Option<SteerOutcome>, CommandError> {
    match cx.engine().steer(sref, steer, Deadline(STEER_DEADLINE)) {
        Ok(o) => Ok(Some(o)),
        Err(EngineError::PaneGone) => {
            let code = match before {
                Some(AgentState::Exited { code }) => Some(*code),
                _ => None,
            };
            let saved = transcript::save(cx, store, row, session, Trigger::Exit);
            store.end_session(session.id, code, saved.as_ref().map(|t| t.digest.as_str()))?;
            if is_exit(steer) {
                return Ok(None);
            }
            let ended = code.map_or_else(
                || "the agent's pane is gone".to_owned(),
                |c| format!("agent exited {c}"),
            );
            Err(CommandError::new(
                ErrorKind::Refused,
                format!("{ended}; relaunch with {}", relaunch(name)),
            ))
        }
        Err(EngineError::NoSession) => Err(CommandError::new(
            ErrorKind::Refused,
            format!("the agent has no pane; relaunch with {}", relaunch(name)),
        )),
        Err(e) => Err(CommandError::new(ErrorKind::Failed, e.to_string())),
    }
}

/// Whether a menu choice was answered; a text steer warns when its submit was not confirmed.
fn answered(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    steer: &Steer,
    outcome: SteerOutcome,
) -> Result<bool, CommandError> {
    match steer {
        Steer::Menu(_) => menu_answered(cx, store, row),
        Steer::Text(_) => {
            if !outcome.submit_checked {
                cx.warn(Warning {
                    code: "steer_unchecked".to_owned(),
                    message: "could not confirm the message was submitted".to_owned(),
                });
            }
            Ok(false)
        }
    }
}

/// Probes the agent once more after a menu choice; warns when it still waits.
fn menu_answered(cx: &Cx, store: &Store, row: &BoxRow) -> Result<bool, CommandError> {
    let waiting = matches!(
        state_of(cx, store, row)?,
        Some(AgentState::NeedsInput { .. })
    );
    if waiting {
        cx.warn(Warning {
            code: "steer_unanswered".to_owned(),
            message: "the agent still waits at the prompt".to_owned(),
        });
    }
    Ok(!waiting)
}

/// What to send for `req` given what the agent shows.
///
/// A blocked agent takes a menu choice only: `--menu N`, or a lone digit as the text.
fn choose(req: &SteerReq, state: Option<&AgentState>) -> Result<Steer, CommandError> {
    let task = req.box_name.as_str();
    let waiting = match state {
        Some(AgentState::NeedsInput { question, .. }) => Some(question),
        _ => None,
    };
    match (req.menu, req.text.as_deref(), waiting) {
        (Some(n), _, Some(_)) => Ok(Steer::Menu(choice(n)?)),
        (Some(_), _, None) => Err(CommandError::new(
            ErrorKind::Refused,
            format!("the agent of {task} is not waiting at a menu"),
        )),
        (None, Some(t), None) => Ok(Steer::Text(t.to_owned())),
        (None, Some(t), Some(w)) => match t.parse::<u8>() {
            Ok(n) if t.len() == 1 => Ok(Steer::Menu(n)),
            _ => Err(CommandError::new(
                ErrorKind::Refused,
                format!(
                    "the agent of {task} is blocked (waiting for: {}); answer with `moor steer {task} --menu N` or open the Workspace with `moor start {task}`",
                    w.as_deref().unwrap_or("input")
                ),
            )),
        },
        (None, None, _) => Err(CommandError::new(ErrorKind::Usage, "nothing to send")),
    }
}

/// A menu number the pane keys can carry.
fn choice(n: u32) -> Result<u8, CommandError> {
    u8::try_from(n)
        .ok()
        .filter(|n| *n > 0)
        .ok_or_else(|| CommandError::new(ErrorKind::Usage, format!("no menu choice {n}")))
}

/// Append the `steered` event; its id.
fn record(
    store: &Store,
    id: BoxId,
    session: &SessionRow,
    message: &str,
    submit_retried: bool,
) -> Result<i64, CommandError> {
    let who = if session.session_ref.is_empty() {
        session.pane.as_deref().unwrap_or_default()
    } else {
        &session.session_ref
    };
    let event = ExtraEvent::Steered {
        session: who.to_owned(),
        message: message.to_owned(),
        submit_retried,
    };
    let applied = store.apply(Decision::new(id).event(event))?;
    applied
        .events
        .first()
        .copied()
        .ok_or_else(|| CommandError::new(ErrorKind::Failed, "the steered event was not recorded"))
}

#[cfg(test)]
mod tests;
