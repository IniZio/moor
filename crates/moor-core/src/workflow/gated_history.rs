//! What a gate line says in the history of the run of its Box: the line itself, the gates it
//! passed and the gate it put in hand. Written with the line, so the history reads in the order
//! things happened, whenever the tick gets to the run.

use super::{HistoryEntry, StepId};
use crate::gate::{passed_gates, status, GateLine, GateStatus, Gates};

/// The history lines for `line`, which came after `before`. A run that waits for a person is
/// put back in hand by its tick, so `run_waits` leaves the gate in hand to that.
#[must_use]
pub fn gate_history(
    gates: &Gates,
    before: &[GateLine],
    line: &GateLine,
    run_waits: bool,
) -> Vec<HistoryEntry> {
    let mut out: Vec<HistoryEntry> = of_line(gates, line).into_iter().collect();
    let after: Vec<GateLine> = before.iter().chain([line]).cloned().collect();
    let (was, now) = (status(gates, before), status(gates, &after));
    let (from, to) = (passed_gates(gates, before), passed_gates(gates, &after));
    out.extend(
        gates
            .iter()
            .skip(from)
            .take(to.saturating_sub(from))
            .map(|g| HistoryEntry::GatePassed {
                step: g.name.clone(),
            }),
    );
    if let (GateStatus::Passed { outcome }, false) =
        (&now, matches!(was, GateStatus::Passed { .. }))
    {
        out.push(HistoryEntry::OutcomeReached {
            outcome: outcome.clone(),
        });
    }
    if let (GateStatus::Open { gate, try_n, .. }, false) = (&now, run_waits) {
        if in_hand(&was) != Some((gate, *try_n)) {
            out.push(HistoryEntry::Entered {
                step: gate.clone(),
                try_n: *try_n,
            });
        }
    }
    out
}

/// The gate and try an open status puts in hand.
const fn in_hand(s: &GateStatus) -> Option<(&StepId, u8)> {
    match s {
        GateStatus::Open { gate, try_n, .. } => Some((gate, *try_n)),
        _ => None,
    }
}

/// The line as the run's history tells it; none for a line that is no event of its own.
fn of_line(gates: &Gates, line: &GateLine) -> Option<HistoryEntry> {
    Some(match line.clone() {
        GateLine::Submitted {
            gate,
            try_n,
            report,
        } => HistoryEntry::Reported {
            step: gate,
            try_n,
            report,
        },
        GateLine::Refused {
            gate,
            try_n,
            missing,
        } => HistoryEntry::GateRefused {
            step: gate,
            try_n,
            missing,
        },
        GateLine::Checked {
            gate,
            try_n,
            check,
            exit,
            tail,
        } => HistoryEntry::Checked {
            check: gates
                .iter()
                .find(|g| g.name == gate)
                .map(|g| g.check_label(check)),
            step: gate,
            try_n,
            exit,
            tail,
        },
        GateLine::Reminded { gate, try_n } => HistoryEntry::Reminded { step: gate, try_n },
        GateLine::Answered {
            gate,
            try_n,
            by,
            answer,
            note,
        } => HistoryEntry::GateAnswered {
            step: gate,
            try_n,
            by,
            answer,
            note: Some(note).filter(|n| !n.is_empty()),
        },
        GateLine::Quiet { .. }
        | GateLine::NoAnswer { .. }
        | GateLine::TryFailed { .. }
        | GateLine::Reopened { .. }
        | GateLine::Held { .. }
        | GateLine::NoneHeld { .. } => return None,
    })
}

#[cfg(test)]
mod tests;
