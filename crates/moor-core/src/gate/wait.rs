//! What a gate that waits for a person puts before them: the owner's yes or no, or a steer that
//! reopens a gate that is stuck.

use serde::{Deserialize, Serialize};

use super::{status, GateLine, GateNext, GateStatus, Gates};
use crate::workflow::{StepId, StepResult};

/// What the person is asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitKind {
    /// Say yes or no to what the agent handed in.
    Approve,
    /// Steer the agent, which reopens the gate with fresh tries.
    Stuck,
}

impl WaitKind {
    /// The kind as the gate's events name it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::Stuck => "stuck",
        }
    }
}

/// One gate that waits for a person, with what they need to read to act.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wait {
    /// The gate.
    pub gate: String,
    /// What is asked.
    pub kind: WaitKind,
    /// How many gate lines were written when the wait began, which later lines that change
    /// nothing leave as it was; it names this wait, so an answer never lands on a later one.
    pub seq: u32,
    /// The try the owner is asked about; none for a stuck gate.
    pub try_n: Option<u32>,
    /// Why the gate is stuck.
    pub why: Option<String>,
    /// The evidence names the agent handed in for the try.
    pub handed: Vec<String>,
    /// What the agent wrote with its report.
    pub note: Option<String>,
    /// The check that failed, as written in the gate.
    pub command: Option<String>,
    /// The end of the failed check's output.
    pub tail: Option<String>,
    /// The evidence names that were not found.
    pub missing: Vec<String>,
    /// What the person can do about it.
    pub hint: String,
}

impl Wait {
    /// What the person is told, in a sentence: what the gate asks of them, or why it is stuck.
    #[must_use]
    pub fn question(&self) -> String {
        let gate = &self.gate;
        match self.kind {
            WaitKind::Approve => {
                let handed = match self.handed.as_slice() {
                    [] => String::new(),
                    list => format!(" The agent handed in {}.", list.join(", ")),
                };
                format!("Gate `{gate}` asks your approval.{handed}")
            }
            WaitKind::Stuck => format!(
                "Gate `{gate}` is stuck: {}",
                self.why.as_deref().unwrap_or("it cannot go on")
            ),
        }
    }
}

/// The wait the gates are at after `lines`; none unless a gate waits for a person.
#[must_use]
pub fn wait(gates: &Gates, lines: &[GateLine]) -> Option<Wait> {
    let now = status(gates, lines);
    let mut began = lines.len();
    while let Some(before) = began.checked_sub(1).and_then(|n| lines.get(..n)) {
        if status(gates, before) != now {
            break;
        }
        began -= 1;
    }
    let seq = u32::try_from(began).unwrap_or(u32::MAX);
    let blank = |gate: &StepId, kind, hint: &str| Wait {
        gate: gate.to_string(),
        kind,
        seq,
        try_n: None,
        why: None,
        handed: Vec::new(),
        note: None,
        command: None,
        tail: None,
        missing: Vec::new(),
        hint: hint.to_owned(),
    };
    match now {
        GateStatus::Open {
            gate,
            try_n,
            next: GateNext::Ask,
        } => {
            let hint =
                "Answer yes to pass the gate, or no to fail this try; a note goes to the agent.";
            let mut w = blank(&gate, WaitKind::Approve, hint);
            w.try_n = Some(u32::from(try_n));
            for line in lines {
                if let GateLine::Submitted {
                    gate: g,
                    try_n: t,
                    report,
                } = line
                {
                    if *g == gate && *t == try_n {
                        w.handed.clone_from(&report.evidence);
                        w.note = Some(report.note.clone()).filter(|n| !n.is_empty());
                    }
                }
            }
            Some(w)
        }
        GateStatus::Stuck { gate, why, .. } => {
            let hint = "Send the agent a message to reopen the gate with fresh tries.";
            let mut w = blank(&gate, WaitKind::Stuck, hint);
            w.why = Some(why);
            if let Some(g) = gates.iter().find(|g| g.name == gate) {
                stuck_by(&mut w, g, lines);
            }
            Some(w)
        }
        GateStatus::Open { .. } | GateStatus::Failed { .. } | GateStatus::Passed => None,
    }
}

/// Fills `w` from the newest line of `gate`, since it was last reopened, that failed a try.
fn stuck_by(w: &mut Wait, gate: &super::Gate, lines: &[GateLine]) {
    let command = |check: u8| gate.checks.get(usize::from(check)).cloned();
    for line in lines.iter().rev() {
        match line {
            GateLine::Reopened { gate: g, .. } if *g == gate.name => return,
            GateLine::Checked {
                gate: g,
                check,
                exit,
                tail,
                ..
            } if *g == gate.name && *exit != 0 => {
                w.command = command(*check);
                w.tail = Some(tail.clone()).filter(|t| !t.trim().is_empty());
                return;
            }
            GateLine::CheckLost { gate: g, check, .. } if *g == gate.name => {
                w.command = command(*check);
                return;
            }
            GateLine::Refused {
                gate: g, missing, ..
            } if *g == gate.name => {
                w.missing.clone_from(missing);
                return;
            }
            GateLine::Submitted {
                gate: g, report, ..
            } if *g == gate.name && report.result == StepResult::Fail => {
                w.note = Some(report.note.clone()).filter(|n| !n.is_empty());
                return;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests;
