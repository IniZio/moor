//! `report_step` from a Box that has gates: the agent hands in the evidence for the gate in hand.
//!
//! The lines are the state. A report that is for the gate in hand, while it waits for the agent,
//! is checked against the Box's artifacts and written as `Submitted` or `Refused`; any other
//! report is answered and writes nothing, since `status()` would ignore the line. The line takes
//! the number after those read, so a report raced by a pass or a repeat is written once or read
//! again.

use moor_core::artifact::ArtifactFile;
use moor_core::decide::workflow::evidence_gate;
use moor_core::gate::{status, Gate, GateLine, GateNext, GateStatus, Gates};
use moor_core::request::StepReportReq;
use moor_core::view::workflow::StepReportValue;
use moor_core::workflow::{StepReport, StepResult};

use super::checks::taken;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::artifacts::index_now;
use crate::store::Store;

/// How often one report reads the lines again after another line took its number.
const TRIES: usize = 3;

/// A reply that tells the agent how to go on.
fn said(reply: impl Into<String>) -> StepReportValue {
    StepReportValue {
        reply: reply.into(),
        error: false,
    }
}

/// A reply that tells the agent why nothing was recorded.
fn refused(reply: impl Into<String>) -> StepReportValue {
    StepReportValue {
        reply: reply.into(),
        error: true,
    }
}

/// What the agent is told when moor runs the gate in hand and a report is not wanted.
const END_TURN: &str = "End your turn. moor runs the checks and sends you the result.";

/// `step-report` for a Box with gates; none when the Box has none.
///
/// # Errors
/// A store failure.
pub(crate) fn run(
    cx: &Cx,
    store: &Store,
    req: &StepReportReq,
) -> Result<Option<StepReportValue>, CommandError> {
    let Some(gates) = store.box_gates(req.box_id)? else {
        return Ok(None);
    };
    for _ in 0..TRIES {
        let lines = store.gate_lines(req.box_id)?;
        if let Some(reply) = settle(cx, store, (&gates, &lines), req)? {
            return Ok(Some(reply));
        }
    }
    Ok(Some(refused(
        "The gates moved while you reported. Call report_step again if the gate is still yours.",
    )))
}

/// Handles `req` against the gates and their lines; none when another line took the number.
fn settle(
    cx: &Cx,
    store: &Store,
    (gates, lines): (&Gates, &[GateLine]),
    req: &StepReportReq,
) -> Result<Option<StepReportValue>, CommandError> {
    let report = &req.report;
    let GateStatus::Open { gate, try_n, next } = status(gates, lines) else {
        return Ok(Some(closed(&status(gates, lines), report)));
    };
    let Some(current) = gates.iter().find(|g| g.name == gate) else {
        return Ok(Some(refused("The Box has no such gate.")));
    };
    if report.step != gate {
        return Ok(Some(elsewhere(gates, current, report)));
    }
    if next != GateNext::Agent {
        return Ok(Some(not_yours(lines, current, (try_n, next), report)));
    }
    let index = match looked_in(cx, store, req) {
        Ok(index) => index,
        Err(reply) => return Ok(Some(reply)),
    };
    let line = match evidence_gate(current.evidence.as_ref(), report, &index) {
        Ok(()) => GateLine::Submitted {
            gate: gate.clone(),
            try_n,
            report: report.clone(),
        },
        Err(missing) => GateLine::Refused {
            gate: gate.clone(),
            try_n,
            missing,
        },
    };
    let n = i64::try_from(lines.len())
        .unwrap_or(i64::MAX)
        .saturating_add(1);
    match store.append_gate_line(req.box_id, n, cx.clock().now(), &line) {
        Ok(()) => {}
        Err(e) if taken(&e) => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    let mut now = lines.to_vec();
    now.push(line.clone());
    Ok(Some(after(gates, &now, &line)))
}

/// The files of the Box's artifacts folder the evidence is checked against, looked up now; only an
/// ok report needs them. The reply when the Box cannot be looked in.
fn looked_in(
    cx: &Cx,
    store: &Store,
    req: &StepReportReq,
) -> Result<Vec<ArtifactFile>, StepReportValue> {
    if req.report.result != StepResult::Ok {
        return Ok(Vec::new());
    }
    let nobody = || refused("moor could not find this Box. Nothing is recorded.");
    let row = store
        .box_row(req.box_id)
        .map_err(|e| {
            refused(format!(
                "moor could not read the Box ({e}). Nothing is recorded; call report_step again."
            ))
        })?
        .ok_or_else(nobody)?;
    index_now(cx, &row).map_err(|why| {
        refused(format!(
            "moor could not look in the Box to check your evidence ({why}). Nothing is \
             recorded; call report_step again."
        ))
    })
}

/// The reply when no gate is open: they have all passed, or the one in hand is stuck or failed.
fn closed(status: &GateStatus, report: &StepReport) -> StepReportValue {
    match status {
        GateStatus::Passed => refused(format!(
            "Every gate has passed, so there is nothing to report for `{}`.",
            report.step
        )),
        GateStatus::Stuck { gate, why, .. } | GateStatus::Failed { gate, why } => refused(format!(
            "Gate `{gate}` is not open: {why}. A person has to take it up. Nothing is recorded."
        )),
        GateStatus::Open { .. } => refused("A gate is open."),
    }
}

/// The reply to a report of a gate that is not the one in hand.
fn elsewhere(gates: &Gates, current: &Gate, report: &StepReport) -> StepReportValue {
    let names: Vec<String> = gates.iter().map(|g| g.name.to_string()).collect();
    let hand = format!(
        "The gate in hand is `{}`. {}",
        current.name,
        current.brief()
    );
    if names.contains(&report.step.to_string()) {
        refused(format!(
            "Gate `{}` is not the gate in hand. {hand}",
            report.step
        ))
    } else {
        refused(format!(
            "This Box has no gate `{}`. Its gates are {}. {hand}",
            report.step,
            names.join(", ")
        ))
    }
}

/// The reply to a report of the gate in hand while moor, not the agent, is to move it: a repeat of
/// the report that put it there is already recorded.
fn not_yours(
    lines: &[GateLine],
    current: &Gate,
    (try_n, next): (u8, GateNext),
    report: &StepReport,
) -> StepReportValue {
    let repeat = lines.iter().any(|l| {
        matches!(l, GateLine::Submitted { gate, try_n: t, report: r }
            if gate == &current.name && *t == try_n && r == report)
    });
    let go_on = match next {
        GateNext::Ask => "End your turn. moor asks the owner and sends you the answer.",
        _ => END_TURN,
    };
    if repeat {
        return said(format!("Already recorded. {go_on}"));
    }
    refused(format!(
        "Gate `{}` is not waiting for a report from you. {go_on} Nothing is recorded.",
        current.name
    ))
}

/// What the agent is told once `line`, the last of `lines`, is written.
fn after(gates: &Gates, lines: &[GateLine], line: &GateLine) -> StepReportValue {
    let now = status(gates, lines);
    match (line, now) {
        (
            GateLine::Refused {
                missing,
                gate,
                try_n,
            },
            now,
        ) => {
            let list = missing.join("; ");
            match now {
                GateStatus::Open {
                    gate: g, try_n: t, ..
                } if &g == gate && t == *try_n => refused(format!(
                    "Refused: {list}. Fix this and call report_step again. A second refusal fails \
                     this try."
                )),
                GateStatus::Open {
                    gate: g, try_n: t, ..
                } if &g == gate => refused(format!(
                    "Refused: {list}. That was the second refusal, so try {try_n} failed. Try {t} \
                     starts: fix this and call report_step again."
                )),
                _ => refused(format!(
                    "Refused: {list}. That was the second refusal, and the gate is out of tries. \
                     A person has to take it up."
                )),
            }
        }
        (GateLine::Submitted { gate, try_n, .. }, now) => submitted(gates, (gate, *try_n), now),
        _ => said("Recorded."),
    }
}

/// What the agent is told once its report of try `try_n` of `gate` is recorded, and the gates
/// stand at `now`.
fn submitted(
    gates: &Gates,
    (gate, try_n): (&moor_core::workflow::StepId, u8),
    now: GateStatus,
) -> StepReportValue {
    match now {
        GateStatus::Passed => said("Recorded. Every gate has passed. There is nothing more to do."),
        GateStatus::Open {
            gate: g,
            try_n: t,
            next,
        } if &g == gate && t == try_n => said(format!(
            "Recorded. {}",
            if next == GateNext::Ask {
                "End your turn. moor asks the owner and sends you the answer."
            } else {
                "End your turn. moor runs the checks and sends you the result."
            }
        )),
        GateStatus::Open {
            gate: g, try_n: t, ..
        } if &g == gate => said(format!(
            "Recorded as a failed try. Try {t} of gate `{g}` starts: call report_step again once \
             the work is fixed."
        )),
        GateStatus::Open { gate: g, next, .. } => {
            let brief = gates
                .iter()
                .find(|x| x.name == g)
                .map(Gate::brief)
                .unwrap_or_default();
            let go_on = match next {
                GateNext::Agent => "Do it, then call report_step.",
                GateNext::RunChecks => "End your turn. moor runs the checks.",
                GateNext::Ask => "End your turn. moor asks the owner.",
            };
            said(format!(
                "Recorded. Gate `{gate}` has passed. Next: {brief} {go_on}"
            ))
        }
        GateStatus::Stuck { why, .. } | GateStatus::Failed { why, .. } => said(format!(
            "Recorded, but the gate cannot go on: {why}. A person has to take it up."
        )),
    }
}
