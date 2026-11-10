//! `steer`: send a message or a menu choice to the Box's agent.
//!
//! The steer holds its Box's turn, which a run in flight leaves free, so a Box can be steered
//! during a run. Each steer waits in the Box's inbox until the turn it starts ends; a text
//! steer to a Box that cannot take it now stays there, queued, and moor sends it later.

use std::time::Duration;

use moor_core::activity::{Activity, AgentState, Source};
use moor_core::decide::transcript::Trigger;
use moor_core::domain::decision::Decision;
use moor_core::domain::inbox::{InboxId, InboxKind};
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::engine::{EngineError, SessionRef, Steer, SteerOutcome};
use moor_core::seam::proc::Deadline;
use moor_core::view::steer::SteerValue;
use moor_core::{BoxId, ErrorDetail, ErrorKind, SteerReq, Warning};

use crate::cx::owners::Cmd;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::activity::{self, session_ref};
use crate::ops::common::stale::{end_read_launch, Ending};
use crate::ops::common::target::{self, Rule};
use crate::ops::common::transcript;
use crate::ops::common::wake::wake_and_revive;
use crate::store::{BoxRow, Launch, LaunchEnd, Store, StoreError};

/// Longest the send may take, pane reads included.
const STEER_DEADLINE: Duration = Duration::from_secs(30);

/// Run `steer`.
///
/// A text steer is queued when the Box sleeps, lost its machine or agent, or another op holds
/// it, and a later pass sends it. A menu choice, an interrupt or `/exit` to an awake Box whose
/// agent exited gets the agent relaunched once, resuming its stored agent session.
///
/// # Errors
/// [`CommandError`] when the Box is unknown or has no live agent, the agent ended and could not
/// be relaunched or ended again, the choice does not fit what the agent shows, or the send failed.
pub(crate) fn run(cx: &Cx, req: &SteerReq) -> Result<SteerValue, CommandError> {
    let store = cx.store()?;
    let found = target::resolve(store, &req.target, Rule::Open)?.ok_or_else(|| {
        CommandError::new(ErrorKind::Failed, format!("no open Box {}", req.target))
    })?;
    if req.menu.is_none() && !req.interrupt && req.text.is_some() && !is_exit_req(req) {
        return queue::steer(cx, store, req, &found);
    }
    let target = (found.id, found.name.as_str());
    if lost(&found) {
        bring_back(cx, &found)?;
    }
    let _turn = cx.turn(target.0, Cmd::Steer)?;
    let mut slot = Slot::new(InboxKind::Steer);
    let sent = steer_awake(cx, store, req, target, &mut slot);
    slot.withdraw(store, target.0)?;
    // A choice, interrupt or `/exit` sent later would act on a screen nobody sees by then.
    if let (Err(_), Some((row, _))) = (&sent, slot.in_doubt(store, target.0)?) {
        store.unconfirmed(target.0, row)?;
    }
    sent
}

/// Steers `target` under its turn, waking it first when asleep and relaunching an agent found
/// gone once.
fn steer_awake(
    cx: &Cx,
    store: &Store,
    req: &SteerReq,
    target: (BoxId, &str),
    slot: &mut Slot,
) -> Result<SteerValue, CommandError> {
    let row = reload(store, target)?;
    if store.live_launch(target.0)?.is_none() && row.asleep() {
        return Err(no_agent(target.1));
    }
    if row.asleep() {
        wake_and_revive(cx, store, &row)?;
    }
    let gone = match attempt(cx, store, req, target, slot)? {
        Attempt::Sent(v) => return Ok(v),
        Attempt::Gone(gone) => gone,
    };
    if req.interrupt {
        return Err(gone.error);
    }
    let mut revived = relaunch::once(cx, store, target, gone)?;
    let mut seen_running = false;
    loop {
        let again = match attempt(cx, store, req, target, slot)? {
            Attempt::Sent(v) => return Ok(v),
            Attempt::Gone(again) => again,
        };
        match revived {
            Revived::Relaunched => return Err(relaunch::ended_again(cx, store, target, &again)?),
            Revived::Running if seen_running => return Err(relaunch::lost_again(target.1)),
            Revived::Running => {
                seen_running = true;
                revived = relaunch::once(cx, store, target, again)?;
            }
        }
    }
}

/// Sends `text` to the live agent of awake Box `row`, the way a steer does.
pub(crate) fn tell(cx: &Cx, store: &Store, row: &BoxRow, text: String) -> Result<(), CommandError> {
    let req = SteerReq {
        target: moor_core::request::BoxTarget::Id(row.id),
        text: Some(text),
        menu: None,
        interrupt: false,
    };
    let mut slot = Slot::new(InboxKind::Note);
    let sent = attempt(cx, store, &req, (row.id, row.name.as_str()), &mut slot);
    slot.withdraw(store, row.id)?;
    match sent? {
        Attempt::Sent(_) => Ok(()),
        Attempt::Gone(gone) => Err(gone.error),
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
    pub(super) launch: Option<Launch>,
    /// The exit code the agent left, when known.
    pub(super) code: Option<i32>,
    /// Why the steer was not sent.
    pub(super) error: CommandError,
}

impl Gone {
    /// The agent of `name` gone from `launch`, having left `code`.
    fn new(name: &str, launch: Option<Launch>, code: Option<i32>) -> Self {
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

    /// Saves the transcript of the gone agent and ends its launch with its Session, or the
    /// current Session when no launch was read. A launch that ended meanwhile with no successor
    /// is left as is, its Session too; false when another op replaced it since it was read.
    pub(super) fn finish(&self, cx: &Cx, store: &Store, id: BoxId) -> Result<bool, CommandError> {
        let Some(launch) = &self.launch else {
            store.end_current_session(id, "exited", self.code)?;
            return Ok(true);
        };
        let saved = store
            .box_row(id)?
            .and_then(|row| transcript::save(cx, store, &row, launch, Trigger::Exit));
        let end = LaunchEnd {
            exit: self.code,
            digest: saved.as_ref().map(|t| t.digest.as_str()),
            session: Some("exited"),
        };
        Ok(end_read_launch(cx, store, (id, launch.id), &end)? == Ending::Ended)
    }
}

/// The refusal for an agent that `ended`.
fn gone_refusal(name: &str, ended: &str) -> CommandError {
    CommandError::new(
        ErrorKind::Refused,
        format!("{ended}; relaunch with {}", relaunch_hint(name)),
    )
    .with_detail(ErrorDetail::AgentGone)
}

/// Sends `req` to the live agent of `target`, or says the agent is gone.
fn attempt(
    cx: &Cx,
    store: &Store,
    req: &SteerReq,
    (id, name): (BoxId, &str),
    slot: &mut Slot,
) -> Result<Attempt, CommandError> {
    let row = reload(store, (id, name))?;
    let Some(session) = store.live_launch(id)? else {
        return Ok(Attempt::Gone(Gone::new(name, None, None)));
    };
    let sref = session_ref(&row, &session).ok_or_else(|| no_agent(name))?;
    let reading = activity::read(cx, store, &row, cx.clock().now(), activity::PROBE_DEADLINE)?;
    if reading.read_failed {
        let msg = format!(
            "could not read the agent of {name}, so its turn could not be told apart; try again"
        );
        return Err(CommandError::new(ErrorKind::Failed, msg));
    }
    let before = reading.activity.clone();
    if before
        .as_ref()
        .is_some_and(|a| a.source == Source::FirstRun)
    {
        return Err(first_run_refusal(name));
    }
    let before = before.map(|a| a.state);
    if let (Some(AgentState::Exited { code }), false) = (&before, is_exit_req(req)) {
        return Ok(Attempt::Gone(Gone::new(name, Some(session), *code)));
    }
    let steer = choose(req, name, before.as_ref())?;
    let sent = match &steer {
        Steer::Text(t) => t.clone(),
        Steer::Menu(n) => n.to_string(),
        Steer::Interrupt => "(interrupt)".to_owned(),
    };
    let (outcome, inbox) = slot.send(cx, store, (id, name), (&sref, &session), (&steer, &sent))?;
    let Some(outcome) = outcome else {
        let code = match before {
            Some(AgentState::Exited { code }) => code,
            _ => None,
        };
        let gone = Gone::new(name, Some(session.clone()), code);
        if !is_exit(&steer) {
            return Ok(Attempt::Gone(gone));
        }
        gone.finish(cx, store, id)?;
        return exited(cx, store, (id, name), &session, (sent, inbox)).map(Attempt::Sent);
    };
    let starts_turn = !(matches!(steer, Steer::Interrupt) && idle(reading.term.as_ref()));
    let mark = Mark {
        turn_before: turn_before(reading.term.as_ref(), &steer).filter(|_| starts_turn),
        status_before: status_before(reading.term.as_ref()).filter(|_| starts_turn),
        submit_retried: outcome.submit_retried,
    };
    let steer_id = record(store, id, &session, &sent, mark, inbox)?;
    if starts_turn {
        open_turn(store, &row)?;
    }
    let answered_menu = answered(cx, store, &row, &steer, outcome)?;
    Ok(Attempt::Sent(delivered(
        name,
        sent,
        answered_menu,
        (steer_id, inbox),
    )))
}

/// The result of an exit steer whose pane was already gone: recorded, with a warning.
fn exited(
    cx: &Cx,
    store: &Store,
    (id, name): (BoxId, &str),
    session: &Launch,
    (sent, inbox): (String, InboxId),
) -> Result<SteerValue, CommandError> {
    let steer = record(store, id, session, &sent, Mark::default(), inbox)?;
    cx.warn(Warning {
        code: "agent_exited".to_owned(),
        message: "the agent exited and its session ended".to_owned(),
    });
    Ok(delivered(name, sent, false, (steer, inbox)))
}

/// The command that starts the agent again.
fn relaunch_hint(name: &str) -> String {
    format!("`moor start {name} --prompt \"...\"`")
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
    .with_detail(ErrorDetail::FirstRunScreen)
}

/// Whether `steer` asks the agent to exit.
fn is_exit(steer: &Steer) -> bool {
    matches!(steer, Steer::Text(t) if t.trim() == "/exit")
}

/// Whether `req` is the text that asks the agent to exit.
fn is_exit_req(req: &SteerReq) -> bool {
    req.menu.is_none() && req.text.as_deref().is_some_and(|t| t.trim() == "/exit")
}

/// Sends `steer`; `None` when the agent has exited.
fn send(cx: &Cx, sref: &SessionRef, steer: &Steer) -> Result<Option<SteerOutcome>, CommandError> {
    match cx.engine().steer(sref, steer, Deadline(STEER_DEADLINE)) {
        Ok(o) => Ok(Some(o)),
        Err(EngineError::AgentGone) => Ok(None),
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
        Steer::Interrupt => Ok(false),
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

/// Move an idle agent's session to working: the steer starts a turn, and the settle that sees
/// that turn end is what tells the requester. A session in any other state is left as it is.
fn open_turn(store: &Store, row: &BoxRow) -> Result<(), CommandError> {
    if row.intent.is_some() || !row.session.is_quiet() {
        return Ok(());
    }
    match store.apply(Decision::new(row.id).session(SessionInput::AgentBusy)) {
        Ok(_) | Err(StoreError::Illegal(_)) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

mod choose;
mod inbox;
mod lost;
mod queue;
mod record;
mod relaunch;

pub(crate) use queue::{deliver, pass, Backoff};

use choose::choose;
use inbox::Slot;
use lost::{bring_back, lost};
use record::{delivered, idle, record, status_before, turn_before, Mark};
use relaunch::Revived;

#[cfg(test)]
mod tests;
