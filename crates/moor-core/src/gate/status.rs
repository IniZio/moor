//! Where a Box stands against its gates: a pure fold over the lines written for them.

use super::check::named;
use super::{Approval, Gate, GateLine, Gates};
use crate::workflow::{Answer, StepId, StepResult};

/// What an open gate waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GateNext {
    /// The agent's report and its evidence.
    Agent,
    /// moor running the gate's checks.
    RunChecks,
    /// The owner's answer.
    Ask,
}

/// Where a Box stands against its gates.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum GateStatus {
    /// A gate is being tried.
    Open {
        /// The gate.
        gate: StepId,
        /// The try, from 1.
        try_n: u8,
        /// What it waits for.
        next: GateNext,
    },
    /// A gate waits for a person: the agent went quiet, or the tries are used up.
    Stuck {
        /// The gate.
        gate: StepId,
        /// The try it was at when it got stuck.
        try_n: u8,
        /// Why.
        why: String,
    },
    /// A gate failed for good.
    Failed {
        /// The gate.
        gate: StepId,
        /// Why.
        why: String,
    },
    /// Every gate passed.
    Passed,
}

impl GateStatus {
    /// Whether the gates can still move: one is open or stuck, rather than failed for good or
    /// passed.
    #[must_use]
    pub const fn can_move(&self) -> bool {
        matches!(self, Self::Open { .. } | Self::Stuck { .. })
    }
}

/// How a gate ended short of passing.
#[derive(Debug)]
enum End {
    /// Waits for a person's steer, at the try given.
    Stuck(String, u8),
    /// Over.
    Failed(String),
}

/// The tries of one gate and the state of the try in hand.
#[derive(Debug, Default)]
struct Attempts {
    /// Tries that failed since the gate opened or was last reopened.
    used: u8,
    /// Refusals in this try.
    refused: u8,
    /// Lost check streams on the check in hand.
    lost: u8,
    /// Checks that passed in this try.
    checks: u8,
    /// Whether the agent was reminded in this try.
    reminded: bool,
    /// Whether an accepted report came in this try.
    submitted: bool,
    /// Whether the owner said yes in this try.
    answered: bool,
}

impl Attempts {
    /// What `gate` waits for in this try; none when the try has passed it.
    fn next(&self, gate: &Gate) -> Option<GateNext> {
        if gate.evidence.is_some() && !self.submitted {
            Some(GateNext::Agent)
        } else if usize::from(self.checks) < gate.checks.len() {
            Some(GateNext::RunChecks)
        } else if gate.approval == Approval::Owner && !self.answered {
            Some(GateNext::Ask)
        } else {
            None
        }
    }

    /// Starts the next try, keeping the count.
    fn restart(&mut self) {
        *self = Self {
            used: self.used,
            ..Self::default()
        };
    }
}

/// A fold over the lines: the gate in hand, its tries, and how it ended if it did.
struct Walk<'a> {
    /// Every gate.
    gates: &'a Gates,
    /// The place of the gate in hand.
    at: usize,
    /// Its tries.
    tries: Attempts,
    /// How it ended short of passing.
    end: Option<End>,
}

/// Where a Box stands, from its `gates` and the `lines` written for them, oldest first.
#[must_use]
pub fn status(gates: &Gates, lines: &[GateLine]) -> GateStatus {
    walk(gates, lines).status()
}

/// The check moor runs next: the gate it belongs to, the try, and its place among the gate's checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DueCheck<'a> {
    /// The gate.
    pub gate: &'a Gate,
    /// The try, from 1.
    pub try_n: u8,
    /// The check's place among the gate's checks, from 0.
    pub check: u8,
}

/// The check due from `gates` and the `lines` written for them; none unless the status is open at
/// `RunChecks`.
#[must_use]
pub fn due_check<'a>(gates: &'a Gates, lines: &[GateLine]) -> Option<DueCheck<'a>> {
    let walk = walk(gates, lines);
    let GateStatus::Open {
        try_n,
        next: GateNext::RunChecks,
        ..
    } = walk.status()
    else {
        return None;
    };
    Some(DueCheck {
        gate: gates.get(walk.at)?,
        try_n,
        check: walk.tries.checks,
    })
}

/// The fold of `lines` over `gates`.
fn walk<'a>(gates: &'a Gates, lines: &[GateLine]) -> Walk<'a> {
    let mut walk = Walk {
        gates,
        at: 0,
        tries: Attempts::default(),
        end: None,
    };
    for line in lines {
        walk.apply(line);
    }
    walk
}

impl Walk<'_> {
    /// The status the lines so far leave.
    fn status(&self) -> GateStatus {
        let Some(gate) = self.gates.get(self.at) else {
            return GateStatus::Passed;
        };
        let name = gate.name.clone();
        match &self.end {
            Some(End::Stuck(why, try_n)) => GateStatus::Stuck {
                gate: name,
                try_n: *try_n,
                why: why.clone(),
            },
            Some(End::Failed(why)) => GateStatus::Failed {
                gate: name,
                why: why.clone(),
            },
            None => self
                .tries
                .next(gate)
                .map_or(GateStatus::Passed, |next| GateStatus::Open {
                    gate: name,
                    try_n: self.tries.used + 1,
                    next,
                }),
        }
    }

    /// Takes one line in.
    fn apply(&mut self, line: &GateLine) {
        let gates = self.gates;
        let Some(gate) = gates.get(self.at) else {
            return;
        };
        if line.gate() != &gate.name {
            return;
        }
        match &self.end {
            Some(End::Stuck(..)) => self.reopen(line),
            Some(End::Failed(_)) => {}
            None => self.take(gate, line),
        }
        self.settle();
    }

    /// A reopening frees a stuck gate with fresh tries.
    fn reopen(&mut self, line: &GateLine) {
        if matches!(line, GateLine::Reopened { .. }) {
            self.end = None;
            self.tries = Attempts::default();
        }
    }

    /// Moves on to the next gate while the one in hand has passed.
    fn settle(&mut self) {
        while self.end.is_none() {
            let Some(gate) = self.gates.get(self.at) else {
                return;
            };
            if self.tries.next(gate).is_some() {
                return;
            }
            self.at += 1;
            self.tries = Attempts::default();
        }
    }

    /// Whether a line of try `try_n` is about the try in hand.
    const fn current(&self, try_n: u8) -> bool {
        try_n == self.tries.used + 1
    }

    /// Whether the try in hand waits for `what` and `try_n` is it.
    fn waits(&self, gate: &Gate, try_n: u8, what: GateNext) -> bool {
        self.current(try_n) && self.tries.next(gate) == Some(what)
    }

    /// Takes in a line of the gate in hand while it is open.
    fn take(&mut self, gate: &Gate, line: &GateLine) {
        match line {
            GateLine::Submitted { try_n, report, .. } => {
                self.submitted(gate, *try_n, report.result);
            }
            GateLine::Refused { try_n, missing, .. } => self.refused(gate, *try_n, missing),
            GateLine::Checked {
                try_n, check, exit, ..
            } => self.checked(gate, (*try_n, *check), *exit),
            GateLine::CheckLost { try_n, check, .. } => self.lost(gate, (*try_n, *check)),
            GateLine::Reminded { try_n, .. } => self.reminded(gate, *try_n),
            GateLine::Quiet { try_n, .. } => self.quiet(gate, *try_n),
            GateLine::Answered {
                try_n,
                answer,
                note,
                ..
            } => self.answered(gate, *try_n, (*answer, note)),
            GateLine::TryFailed { try_n, why, .. } => {
                if self.current(*try_n) {
                    self.fail(gate, why);
                }
            }
            GateLine::Reopened { .. } => {}
        }
    }

    /// The agent reported.
    fn submitted(&mut self, gate: &Gate, try_n: u8, result: StepResult) {
        if !self.waits(gate, try_n, GateNext::Agent) {
            return;
        }
        match result {
            StepResult::Ok => self.tries.submitted = true,
            StepResult::Fail => self.fail(gate, "the agent reported failure"),
        }
    }

    /// A report was refused; the second one in a try fails it.
    fn refused(&mut self, gate: &Gate, try_n: u8, missing: &[String]) {
        if !self.waits(gate, try_n, GateNext::Agent) {
            return;
        }
        self.tries.refused += 1;
        if self.tries.refused >= 2 {
            self.fail(
                gate,
                &format!("evidence refused twice, missing: {}", missing.join(", ")),
            );
        }
    }

    /// A check ran; the first failure fails the try.
    fn checked(&mut self, gate: &Gate, (try_n, check): (u8, u8), exit: i32) {
        if !self.waits(gate, try_n, GateNext::RunChecks) || check != self.tries.checks {
            return;
        }
        if exit == 0 {
            self.tries.checks += 1;
            self.tries.lost = 0;
        } else {
            self.fail(gate, &format!("{} exited {exit}", named(gate, check)));
        }
    }

    /// A check's stream was lost; losing it twice fails the try.
    fn lost(&mut self, gate: &Gate, (try_n, check): (u8, u8)) {
        if !self.waits(gate, try_n, GateNext::RunChecks) || check != self.tries.checks {
            return;
        }
        self.tries.lost += 1;
        if self.tries.lost >= 2 {
            self.fail(
                gate,
                &format!("{} lost its output twice", named(gate, check)),
            );
        }
    }

    /// The agent was reminded.
    fn reminded(&mut self, gate: &Gate, try_n: u8) {
        if self.waits(gate, try_n, GateNext::Agent) {
            self.tries.reminded = true;
        }
    }

    /// The agent was quiet again after a reminder.
    fn quiet(&mut self, gate: &Gate, try_n: u8) {
        if self.waits(gate, try_n, GateNext::Agent) && self.tries.reminded {
            self.end = Some(End::Stuck("the agent went quiet".to_owned(), try_n));
        }
    }

    /// The owner answered the try `try_n`; a no on the last try ends the gate. An answer for
    /// another try than the one that asks is a replay and changes nothing.
    fn answered(&mut self, gate: &Gate, try_n: u8, (answer, note): (Answer, &str)) {
        if !self.waits(gate, try_n, GateNext::Ask) {
            return;
        }
        if answer == Answer::Yes {
            self.tries.answered = true;
            return;
        }
        let why = if note.is_empty() {
            "the owner said no".to_owned()
        } else {
            format!("the owner said no: {note}")
        };
        if self.tries.used + 1 >= gate.max_tries.get() {
            self.end = Some(End::Failed(why));
        } else {
            self.fail(gate, &why);
        }
    }

    /// The try in hand failed: the next one starts, or the tries are used up.
    fn fail(&mut self, gate: &Gate, why: &str) {
        self.tries.used += 1;
        if self.tries.used >= gate.max_tries.get() {
            self.end = Some(End::Stuck(format!("out of tries: {why}"), self.tries.used));
        } else {
            self.tries.restart();
        }
    }
}
