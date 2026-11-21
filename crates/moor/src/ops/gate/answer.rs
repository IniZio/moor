//! `gate-answer`: the owner's yes or no for the gate of a Box that asks for it.
//!
//! The lines are the state. The answer is a line stamped with the gate and try it was read at, so
//! an answer meant for an earlier try changes nothing, and it takes the number after the lines it
//! read: of two answers only the first writes, and the other reads the lines again and finds the
//! gate moved. The line is written before the agent is told, as with a failed check, so a no that
//! could not be steered is steered by the next pass.

use std::fmt::Write as _;

use moor_core::gate::{status, wait, GateLine, GateNext, GateStatus, WaitKind};
use moor_core::progress::gate_ask_id;
use moor_core::view::gate::GateAnswerValue;
use moor_core::view::steer::Delivery;
use moor_core::workflow::{Answer, NOTE_MAX};
use moor_core::{ErrorKind, GateAnswerReq};

use super::checks::{taken, tell};
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::authz::NOT_YOURS;
use crate::ops::common::target::{self, Rule};
use crate::store::BoxRow;

/// How often one answer reads the lines again after another line took its number.
const TRIES: usize = 3;

/// A refusal the caller can act on.
fn refused(why: impl std::fmt::Display) -> CommandError {
    CommandError::new(ErrorKind::Refused, why.to_string())
}

/// The note of `req`, trimmed, none when it says nothing.
fn note_of(req: &GateAnswerReq) -> Result<Option<&str>, CommandError> {
    let note = req.note.as_deref().map(str::trim).filter(|n| !n.is_empty());
    if note.is_some_and(|n| n.len() > NOTE_MAX) {
        return Err(CommandError::new(
            ErrorKind::Usage,
            format!("the note is at most {NOTE_MAX} bytes"),
        ));
    }
    Ok(note)
}

/// The open Box `req` names, when the caller may act on it.
fn owned_box(cx: &Cx, req: &GateAnswerReq) -> Result<BoxRow, CommandError> {
    let row = target::resolve(cx.store()?, &req.target, Rule::Open)?.ok_or_else(|| {
        CommandError::new(ErrorKind::Failed, format!("no open Box {}", req.target))
    })?;
    if cx.viewer().may_act_on(row.user) {
        Ok(row)
    } else {
        Err(refused(NOT_YOURS))
    }
}

/// Answers the gate of the Box `req` names that asks its owner.
///
/// # Errors
/// A usage error for a note over [`NOTE_MAX`]; a refusal when the Box is not the caller's, has no
/// gate that asks, or asks something else than `req.ask`; a store failure.
pub(crate) fn run(cx: &Cx, req: &GateAnswerReq) -> Result<GateAnswerValue, CommandError> {
    let store = cx.store()?;
    let note = note_of(req)?;
    let row = owned_box(cx, req)?;
    let name = row.name.as_str();
    let Some(gates) = store.box_gates(row.id)? else {
        return Err(refused(format!(
            "Box {name} has no gates, so nothing asks you"
        )));
    };
    let by = cx.requester().map_or_else(
        |_| "a person".to_owned(),
        |(who, _)| who.as_str().to_owned(),
    );
    for _ in 0..TRIES {
        let lines = store.gate_lines(row.id)?;
        let GateStatus::Open {
            gate,
            try_n,
            next: GateNext::Ask,
        } = status(&gates, &lines)
        else {
            return Err(refused(nothing_asks(name, &status(&gates, &lines))));
        };
        let asked = wait(&gates, &lines).filter(|w| w.kind == WaitKind::Approve);
        if let (Some(ask), Some(asked)) = (&req.ask, &asked) {
            if *ask != gate_ask_id(asked) {
                return Err(refused(format!(
                    "gate {gate} of Box {name} asks something else by now; read it and answer again"
                )));
            }
        }
        let line = GateLine::Answered {
            gate: gate.clone(),
            try_n,
            by: by.clone(),
            answer: req.answer,
            note: note.unwrap_or_default().to_owned(),
        };
        let n = i64::try_from(lines.len())
            .unwrap_or(i64::MAX)
            .saturating_add(1);
        match store.append_gate_line(row.id, n, cx.clock().now(), &line) {
            Ok(()) => return Ok(answered(cx, &row, &gates, req.answer, gate.as_str())),
            Err(e) if taken(&e) => {}
            Err(e) => return Err(e.into()),
        }
    }
    Err(refused(format!(
        "the gates of Box {name} moved while you answered; read what they ask and answer again"
    )))
}

/// What the gates of Box `name` say when none of them asks for an answer.
fn nothing_asks(name: &str, status: &GateStatus) -> String {
    match status {
        GateStatus::Passed => format!("every gate of Box {name} has passed, so nothing asks you"),
        GateStatus::Failed { gate, why } => {
            format!("gate {gate} of Box {name} failed: {why}. Nothing asks you")
        }
        GateStatus::Stuck { gate, why } => format!(
            "gate {gate} of Box {name} is stuck: {why}. Nothing asks for an answer; steer its \
             agent to reopen the gate"
        ),
        GateStatus::Open { gate, next, .. } => {
            let waits = match next {
                GateNext::Agent => "the agent's evidence",
                GateNext::RunChecks => "its checks",
                GateNext::Ask => "your answer",
            };
            format!("gate {gate} of Box {name} waits for {waits}, not for an answer from you")
        }
    }
}

/// What the answer did, once its line is written; the agent is told of a no that leaves tries.
fn answered(
    cx: &Cx,
    row: &BoxRow,
    gates: &moor_core::gate::Gates,
    answer: Answer,
    gate: &str,
) -> GateAnswerValue {
    let Ok((store, lines)) = cx
        .store()
        .map_err(|_| ())
        .and_then(|s| Ok((s, s.gate_lines(row.id).map_err(|_| ())?)))
    else {
        return value(row, gate, "recorded".to_owned());
    };
    let now = status(gates, &lines);
    let mut outcome = match (&now, answer) {
        (GateStatus::Passed, _) => format!("gate {gate} passed, and every gate has passed"),
        (GateStatus::Failed { why, .. }, _) => format!("gate {gate} failed: {why}"),
        (GateStatus::Open { gate: g, try_n, .. }, Answer::No) if g.as_str() == gate => {
            format!("gate {gate} failed this try; try {try_n} starts")
        }
        (GateStatus::Open { gate: g, .. }, _) => format!("gate {gate} passed; gate {g} is next"),
        (GateStatus::Stuck { why, .. }, _) => format!("gate {gate} is stuck: {why}"),
    };
    if matches!(now, GateStatus::Open { .. }) && answer == Answer::No {
        match tell(cx, store, row.id) {
            Ok(Some(sent)) if sent.delivery == Delivery::Delivered => {
                outcome.push_str(", and the agent was told your no");
            }
            Ok(Some(_)) => outcome
                .push_str(", and your no is queued: moor sends it once the agent can take it"),
            Ok(None) => outcome.push_str(", and moor sends the agent your no"),
            Err(e) => {
                let _ = write!(
                    outcome,
                    ", but the agent could not be told ({}); moor tells it again",
                    e.error
                );
            }
        }
    }
    value(row, gate, outcome)
}

/// The value of an answer to `gate` of `row`.
fn value(row: &BoxRow, gate: &str, outcome: String) -> GateAnswerValue {
    GateAnswerValue {
        box_name: row.name.as_str().to_owned(),
        gate: gate.to_owned(),
        outcome,
    }
}
