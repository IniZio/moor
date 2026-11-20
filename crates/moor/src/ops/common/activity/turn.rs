//! The `turn_done` event: the agent finished the turn a sent inbox row started.

use std::time::Duration;

use moor_core::activity::{stalled_for, Activity, AgentState, Source, Status, STARTUP_GRACE};
use moor_core::decide::turn::{turn_done, SteerMark, TermTurn, TurnFacts, TurnSignal};
use moor_core::domain::decision::Field;
use moor_core::domain::event::TurnOutcome;
use moor_core::domain::inbox::{Delivery, SendMark};
use moor_core::seam::clock::Ts;
use moor_core::view::LaunchFailure;

use super::{PaneLiveness, Reading};
use crate::cx::owners::Cmd;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::stale::unless_stale;
use crate::ops::common::view::rfc3339;
use crate::ops::start::{launch_failure_json, record_launch_failure, LAUNCH_TAIL_LINES};
use crate::store::{BoxRow, InboxRow, LaunchEnd, Store};

/// Answer the last sent inbox row once its turn is over: delete it and every row sent before it,
/// append a `turn_done` event for each, and make the Box's archive due.
///
/// A row is answered once, since answering deletes it. It is answered under the Box's turn,
/// before any state step or session end so neither can lose it. While another operation holds
/// the turn nothing is written and the next pass reads the ended turn again; true then, so the
/// caller leaves the session alone until the turn end is on record.
pub(super) fn note_turn_done(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    reading: &Reading,
    now: Ts,
) -> Result<bool, CommandError> {
    let Some(open) = store.sent(row.id)?.pop() else {
        return Ok(false);
    };
    let Some((outcome, untaken)) = answer(reading, &open, now) else {
        return Ok(false);
    };
    let Some(_turn) = cx.try_turn(row.id, Cmd::Settle) else {
        return Ok(true);
    };
    store.answer(row.id, open.id, outcome)?;
    if untaken {
        fail_untaken(cx, store, row, reading, now)?;
    }
    Ok(false)
}

/// Ends the launch of an agent that never took up its first prompt and records it as a failed
/// launch, with the last lines of its screen, which show why when the agent refused the text.
/// No turn will ever come of that prompt, so nothing should wait on one.
fn fail_untaken(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    reading: &Reading,
    now: Ts,
) -> Result<(), CommandError> {
    let Some(launch) = reading.launch else {
        return Ok(());
    };
    let screen = reading.pane_text.as_deref().unwrap_or("");
    let lines: Vec<&str> = screen.lines().filter(|l| !l.trim().is_empty()).collect();
    let tail = lines
        .get(lines.len().saturating_sub(LAUNCH_TAIL_LINES)..)
        .unwrap_or_default()
        .join("\n");
    let failure = LaunchFailure {
        step: "prompt".to_owned(),
        message: format!(
            "the agent never began on its first prompt in {} s; the agent may have refused it",
            STARTUP_GRACE.as_secs()
        ),
        tail: Some(tail).filter(|t| !t.is_empty()),
        at: rfc3339(now),
    };
    if unless_stale(cx, store.end_launch(row.id, launch, &LaunchEnd::default()))?.is_none() {
        return Ok(());
    }
    let column = Field::Set(launch_failure_json(&failure)?);
    record_launch_failure(store, row.id, column, Some(&failure))
}

/// Whether the last sent row's turn is still open by `reading`.
pub(crate) fn turn_open(
    store: &Store,
    row: &BoxRow,
    reading: &Reading,
    now: Ts,
) -> Result<bool, CommandError> {
    Ok(store
        .sent(row.id)?
        .last()
        .is_some_and(|open| answer(reading, open, now).is_none()))
}

/// The answer `reading` gives to sent row `open`, when its turn is over, and whether the row was
/// a first prompt the agent never took up.
fn answer(reading: &Reading, open: &InboxRow, now: Ts) -> Option<(TurnOutcome, bool)> {
    let Delivery::Sent { launch, at, mark } = open.delivery else {
        return None;
    };
    match reading.launch {
        Some(read) if read < launch => return None,
        Some(read) if read > launch => return Some((TurnOutcome::Exited, false)),
        _ => {}
    }
    let facts = TurnFacts {
        steer: Some(SteerMark {
            id: open.id.get(),
            turn_before: mark.turn_before,
            status_before: mark.status_before,
        }),
        term: reading.term.as_ref().map(|t| TermTurn {
            signal: match t.status {
                Some(Status::Idle | Status::Error { .. }) => TurnSignal::Idle,
                Some(Status::Blocked { .. }) => TurnSignal::Blocked,
                Some(Status::Working { .. }) | None => TurnSignal::Other,
            },
            turn: t.turn,
            status_at: t.status_at.map(Ts::as_millis),
        }),
        exited: reading.pane == PaneLiveness::Exited,
    };
    let sent = Ts::from_millis(at);
    let ended = turn_done(&facts)
        .map(|(_, outcome)| outcome)
        .or_else(|| taken_into_running(reading, mark, sent, now));
    if let Some(outcome) = ended {
        return Some((outcome, false));
    }
    if let Some(outcome) = never_taken(reading, mark, sent, now) {
        return Some((outcome, true));
    }
    never_began(reading, sent, now).map(|outcome| (outcome, false))
}

/// How long the agent must stay idle after the turn a steer was typed into ends before that turn
/// counts as the steer's. A message the turn left over starts the next turn at once.
const LEFT_OVER_GRACE: Duration = Duration::from_secs(15);

/// Ends a row typed at `sent` while the agent worked, once the turn running then has ended after
/// `sent` and the agent has stayed idle past [`LEFT_OVER_GRACE`]. Claude Code takes a message
/// typed mid-turn into that turn, so its ended-turn count rises once for the turn and every
/// message typed into it. A send to a working agent records no status and counts the running
/// turn's end, at least 1, as `turn_before`.
fn taken_into_running(reading: &Reading, mark: SendMark, sent: Ts, now: Ts) -> Option<TurnOutcome> {
    let running_ends_at = mark
        .turn_before
        .filter(|b| *b >= 1 && mark.status_before.is_none())?;
    let term = reading.term.as_ref()?;
    let idle = matches!(term.status, Some(Status::Idle | Status::Error { .. }));
    let ended = term.turn >= running_ends_at;
    let grace = i64::try_from(LEFT_OVER_GRACE.as_millis()).unwrap_or(i64::MAX);
    let settled = term.status_at.is_some_and(|at| {
        at.as_millis() > sent.as_millis() && now.as_millis().saturating_sub(at.as_millis()) >= grace
    });
    (idle && ended && settled).then_some(TurnOutcome::Idle)
}

/// Ends a first prompt the agent never took up: sent before any turn ended, with the agent still
/// idle at turn 0 once [`STARTUP_GRACE`] has passed since `sent`, as its activity then reads.
fn never_taken(reading: &Reading, mark: SendMark, sent: Ts, now: Ts) -> Option<TurnOutcome> {
    let first = mark.turn_before == Some(0) && mark.status_before.is_none();
    let idle = matches!(
        reading.activity,
        Some(Activity {
            state: AgentState::Idle,
            source: Source::Status,
        })
    );
    let fresh = reading.term.as_ref().is_some_and(|t| t.turn == 0);
    let past = now.as_millis() >= sent.saturating_add(STARTUP_GRACE).as_millis();
    (first && idle && fresh && past).then_some(TurnOutcome::Idle)
}

/// Ends a row whose turn never showed: its text never reached the agent as a prompt. An agent
/// read idle, its status unchanged since before a row sent at `sent` that long ago, is not
/// working on it.
fn never_began(reading: &Reading, sent: Ts, now: Ts) -> Option<TurnOutcome> {
    let read_idle = matches!(
        reading.activity,
        Some(Activity {
            state: AgentState::Idle | AgentState::Asked { .. },
            source: Source::Status,
        })
    );
    let at = reading
        .term
        .as_ref()
        .and_then(|t| t.status_at)
        .filter(|_| read_idle)?;
    let old = stalled_for(sent, now).is_some();
    let quiet = stalled_for(at, now).is_some();
    (old && quiet).then_some(TurnOutcome::Idle)
}
