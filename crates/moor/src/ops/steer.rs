//! `steer`: send a message or a menu choice to the Box's agent.
//!
//! The Box lock is taken only to wake an asleep Box, so a Box can be steered during a run.

use std::time::Duration;

use moor_core::activity::{Activity, AgentState, Source};
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
use crate::ops::common::target::{self, Rule};
use crate::ops::common::transcript;
use crate::ops::common::wake::wake_and_revive;
use crate::store::{BoxRow, SessionRow, Store};

/// Longest the send may take, pane reads included.
const STEER_DEADLINE: Duration = Duration::from_secs(30);

/// Run `steer`.
///
/// An awake Box whose agent exited gets the agent relaunched once, resuming its stored agent
/// session, and the steer goes to the relaunched agent.
///
/// # Errors
/// [`CommandError`] when the Box is unknown or has no live agent, the agent ended and could not
/// be relaunched or ended again, the choice does not fit what the agent shows, or the send failed.
pub(crate) fn run(cx: &Cx, req: &SteerReq) -> Result<SteerValue, CommandError> {
    let store = cx.store()?;
    let found = target::resolve(store, &req.target, Rule::Open)?.ok_or_else(|| {
        CommandError::new(ErrorKind::Failed, format!("no open Box {}", req.target))
    })?;
    let target = (found.id, found.name.as_str());
    let row = reload(store, target)?;
    if store.live_session(target.0)?.is_none() && asleep(&row) {
        return Err(no_agent(target.1));
    }
    if asleep(&row) {
        let _held = BoxLock::acquire(
            cx.clock(),
            cx.state_dir()?,
            target.0,
            "steer",
            boxlock::WAIT,
        )?;
        wake_and_revive(cx, store, &reload(store, target)?)?;
    }
    let gone = match attempt(cx, store, req, target)? {
        Attempt::Sent(v) => return Ok(v),
        Attempt::Gone(gone) => gone,
    };
    relaunch::once(cx, store, target, gone)?;
    match attempt(cx, store, req, target)? {
        Attempt::Sent(v) => Ok(v),
        Attempt::Gone(again) => Err(relaunch::ended_again(cx, store, target, &again)?),
    }
}

/// The refusal for a Box with no agent to steer.
fn no_agent(name: &str) -> CommandError {
    CommandError::new(
        ErrorKind::Refused,
        format!(
            "no live agent session for Box {name}; start it with {}",
            relaunch_hint(name)
        ),
    )
}

/// The Box row of `id`.
fn reload(store: &Store, (id, name): (BoxId, &str)) -> Result<BoxRow, CommandError> {
    store
        .box_row(id)?
        .ok_or_else(|| CommandError::new(ErrorKind::Failed, format!("no Box {name}")))
}

/// What one try at the send came to.
enum Attempt {
    /// The agent took the steer.
    Sent(SteerValue),
    /// The agent is gone; nothing was sent.
    Gone(Gone),
}

/// An agent found gone, with the refusal a steer gives when it is not relaunched.
pub(super) struct Gone {
    /// The launch the agent ran in, when it has not ended yet.
    pub(super) launch: Option<SessionRow>,
    /// The exit code the agent left, when known.
    pub(super) code: Option<i32>,
    /// Why the steer was not sent.
    pub(super) error: CommandError,
}

impl Gone {
    /// The agent of `name` gone from `launch`, having left `code`.
    fn new(name: &str, launch: Option<SessionRow>, code: Option<i32>) -> Self {
        let error = match (&launch, code) {
            (None, _) => no_agent(name),
            (Some(_), None) => gone_refusal(name, "the agent's pane is gone"),
            (Some(_), Some(c)) => gone_refusal(name, &format!("agent exited {c}")),
        };
        Self {
            launch,
            code,
            error,
        }
    }

    /// Saves the transcript of the gone agent, ends its launch and ends the Session.
    pub(super) fn finish(&self, cx: &Cx, store: &Store, id: BoxId) -> Result<(), CommandError> {
        let live = store.live_session(id)?;
        let launch = self
            .launch
            .as_ref()
            .filter(|l| live.is_some_and(|v| v.id == l.id));
        if let (Some(launch), Some(row)) = (launch, store.box_row(id)?) {
            let saved = transcript::save(cx, store, &row, launch, Trigger::Exit);
            store.end_session(
                launch.id,
                self.code,
                saved.as_ref().map(|t| t.digest.as_str()),
            )?;
        }
        store.end_current_session(id, "exited", self.code)?;
        Ok(())
    }
}

/// The refusal for an agent that `ended`.
fn gone_refusal(name: &str, ended: &str) -> CommandError {
    CommandError::new(
        ErrorKind::Refused,
        format!("{ended}; relaunch with {}", relaunch_hint(name)),
    )
}

/// Sends `req` to the live agent of `target`, or says the agent is gone.
fn attempt(
    cx: &Cx,
    store: &Store,
    req: &SteerReq,
    (id, name): (BoxId, &str),
) -> Result<Attempt, CommandError> {
    let row = reload(store, (id, name))?;
    let Some(session) = store.live_session(id)? else {
        return Ok(Attempt::Gone(Gone::new(name, None, None)));
    };
    let sref = session_ref(&row, session.session_ref.clone(), session.pane.clone())
        .ok_or_else(|| no_agent(name))?;
    let before = activity_of(cx, store, &row)?;
    if before.as_ref().is_some_and(|a| a.source == Source::FirstRun) {
        return Err(first_run_refusal(name));
    }
    let before = before.map(|a| a.state);
    if let (Some(AgentState::Exited { code }), false) = (&before, is_exit_req(req)) {
        return Ok(Attempt::Gone(Gone::new(name, Some(session), Some(*code))));
    }
    let steer = choose(req, name, before.as_ref())?;
    let sent = match &steer {
        Steer::Text(t) => t.clone(),
        Steer::Menu(n) => n.to_string(),
    };
    let Some(outcome) = send(cx, name, &sref, &steer)? else {
        let code = if let Some(AgentState::Exited { code }) = before {
            Some(code)
        } else {
            None
        };
        let gone = Gone::new(name, Some(session.clone()), code);
        if !is_exit(&steer) {
            return Ok(Attempt::Gone(gone));
        }
        gone.finish(cx, store, id)?;
        return exited(cx, store, (id, name), &session, sent).map(Attempt::Sent);
    };
    let steer_id = record(store, id, &session, &sent, outcome.submit_retried)?;
    let answered_menu = answered(cx, store, &row, &steer, outcome)?;
    Ok(Attempt::Sent(SteerValue {
        box_name: name.to_owned(),
        sent,
        answered_menu,
        steer: steer_id,
    }))
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
        box_name: name.to_owned(),
        sent,
        answered_menu: false,
        steer,
    })
}

/// Whether the Box or its machine is asleep.
fn asleep(row: &BoxRow) -> bool {
    row.lifecycle == BoxLifecycle::Asleep
        || row
            .boxed
            .as_ref()
            .is_some_and(|b| b.state == BoxState::Asleep)
}

/// The command that starts the agent again.
fn relaunch_hint(name: &str) -> String {
    format!("`moor start {name} --goal \"...\"`")
}

/// The agent's state now; none when the Box has no agent to read.
fn state_of(cx: &Cx, store: &Store, row: &BoxRow) -> Result<Option<AgentState>, CommandError> {
    Ok(activity_of(cx, store, row)?.map(|a| a.state))
}

/// The agent's activity now; none when the Box has no agent to read.
fn activity_of(cx: &Cx, store: &Store, row: &BoxRow) -> Result<Option<Activity>, CommandError> {
    let reading = activity::read(cx, store, row, cx.clock().now(), activity::PROBE_DEADLINE)?;
    Ok(reading.activity)
}

/// The refusal for an agent stopped at a first-run screen: a key typed there may pick an option
/// that ends the agent, so the Box is started again instead.
fn first_run_refusal(name: &str) -> CommandError {
    CommandError::new(
        ErrorKind::Refused,
        format!(
            "the agent of {name} stopped at a first-run screen that moor's pre-seed should have skipped; this is a moor bug; end the Box with `moor end {name}` and start it again"
        ),
    )
}

/// Whether `steer` asks the agent to exit.
fn is_exit(steer: &Steer) -> bool {
    matches!(steer, Steer::Text(t) if t.trim() == "/exit")
}

/// Whether `req` is the text that asks the agent to exit.
fn is_exit_req(req: &SteerReq) -> bool {
    req.menu.is_none() && req.text.as_deref().is_some_and(|t| t.trim() == "/exit")
}

/// Sends `steer`; `None` when the agent's pane is gone.
fn send(
    cx: &Cx,
    name: &str,
    sref: &SessionRef,
    steer: &Steer,
) -> Result<Option<SteerOutcome>, CommandError> {
    match cx.engine().steer(sref, steer, Deadline(STEER_DEADLINE)) {
        Ok(o) => Ok(Some(o)),
        Err(EngineError::PaneGone) => Ok(None),
        Err(EngineError::NoSession) => Err(CommandError::new(
            ErrorKind::Refused,
            format!(
                "the agent has no pane; relaunch with {}",
                relaunch_hint(name)
            ),
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
fn choose(
    req: &SteerReq,
    box_name: &str,
    state: Option<&AgentState>,
) -> Result<Steer, CommandError> {
    let waiting = match state {
        Some(AgentState::NeedsInput { question, .. }) => Some(question),
        _ => None,
    };
    match (req.menu, req.text.as_deref(), waiting) {
        (Some(n), _, Some(_)) => Ok(Steer::Menu(choice(n)?)),
        (Some(_), _, None) => Err(CommandError::new(
            ErrorKind::Refused,
            format!("the agent of {box_name} is not waiting at a menu"),
        )),
        (None, Some(t), None) => Ok(Steer::Text(t.to_owned())),
        (None, Some(t), Some(w)) => match t.parse::<u8>() {
            Ok(n) if t.len() == 1 => Ok(Steer::Menu(n)),
            _ => Err(CommandError::new(
                ErrorKind::Refused,
                format!(
                    "the agent of {box_name} is blocked (waiting for: {}); answer with `moor steer {box_name} --menu N` or open the Workspace with `moor start {box_name}`",
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

mod relaunch;

#[cfg(test)]
mod tests;
