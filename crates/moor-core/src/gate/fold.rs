//! The fold behind [`super::status()`]: the stage in hand, its tries, and how it ended. The
//! stages are the gates in order, then the closing stage, where moor decides the outcome.

use super::check::named;
use super::outcome::CLOSING_TRIES;
use super::{
    closing, Approval, CheckOf, Gate, GateLine, GateNext, GateStatus, Gates, NoAnswerCause,
    Outcome, OutcomeName, Unanswered,
};
use crate::workflow::{Answer, StepId, StepResult};

/// How a gate ended short of passing.
#[derive(Debug)]
enum End {
    /// Waits for a person's steer.
    Stuck {
        /// Why.
        why: String,
        /// The try it was at.
        try_n: u8,
        /// The check moor last got no answer from, when that is why.
        unanswered: Option<NoAnswerCause>,
    },
    /// Over.
    Failed(String),
}

/// The tries of one gate and the state of the try in hand.
#[derive(Debug, Default)]
pub(super) struct Attempts {
    /// Tries that failed since the gate opened or was last reopened.
    used: u8,
    /// Refusals in this try.
    refused: u8,
    /// Checks in a row moor got no answer from in this try.
    unanswered: u8,
    /// Checks that passed in this try.
    pub(super) checks: u8,
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

/// A fold over the lines: the stage in hand, its tries, and how it ended if it did.
pub(super) struct Walk<'a> {
    /// Every gate and outcome.
    gates: &'a Gates,
    /// The place of the stage in hand: a gate, or the closing stage at the number of gates.
    pub(super) at: usize,
    /// Its tries.
    pub(super) tries: Attempts,
    /// How it ended short of passing.
    end: Option<End>,
    /// The outcome that held, which ends the fold.
    held: Option<&'a Outcome>,
}

/// The fold of `lines` over `gates`.
pub(super) fn walk<'a>(gates: &'a Gates, lines: &[GateLine]) -> Walk<'a> {
    let mut walk = Walk {
        gates,
        at: 0,
        tries: Attempts::default(),
        end: None,
        held: None,
    };
    walk.settle();
    for line in lines {
        walk.apply(line);
    }
    walk
}

impl Walk<'_> {
    /// The status the lines so far leave.
    pub(super) fn status(&self) -> GateStatus {
        if let Some(o) = self.held {
            return GateStatus::Passed {
                outcome: o.verdict(),
            };
        }
        let (name, next) = self.gates.get(self.at).map_or_else(
            || (closing(), Some(GateNext::Decide)),
            |g| (g.name.clone(), self.tries.next(g)),
        );
        match &self.end {
            Some(End::Stuck {
                why,
                try_n,
                unanswered,
            }) => GateStatus::Stuck {
                gate: name,
                try_n: *try_n,
                why: why.clone(),
                unanswered: unanswered.clone(),
            },
            Some(End::Failed(why)) => GateStatus::Failed {
                gate: name,
                why: why.clone(),
            },
            None => GateStatus::Open {
                gate: name,
                try_n: self.tries.used + 1,
                next: next.unwrap_or(GateNext::Decide),
            },
        }
    }

    /// Takes one line in.
    fn apply(&mut self, line: &GateLine) {
        if self.held.is_some() {
            return;
        }
        let gates = self.gates;
        let gate = gates.get(self.at);
        if gate.map_or_else(closing, |g| g.name.clone()) != *line.gate() {
            return;
        }
        if self.end.is_none()
            && !matches!(line, GateLine::NoAnswer { .. })
            && line.try_n().is_some_and(|t| self.current(t))
        {
            self.tries.unanswered = 0;
        }
        match (&self.end, gate) {
            (Some(End::Stuck { .. }), _) => self.reopen(line),
            (Some(End::Failed(_)), _) => {}
            (None, Some(gate)) => self.take(gate, line),
            (None, None) => self.decided(line),
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

    /// Moves on while the stage in hand has passed, so no passed gate is left in hand. The closing
    /// stage settles at once on a first outcome with no check, which always holds.
    fn settle(&mut self) {
        let gates = self.gates;
        while self.end.is_none() && self.held.is_none() {
            let Some(gate) = gates.get(self.at) else {
                self.held = gates.outcomes().first().filter(|o| o.checks.is_empty());
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
            GateLine::NoAnswer {
                try_n, check, why, ..
            } => {
                if self.waits(gate, *try_n, GateNext::Agent)
                    || self.waits(gate, *try_n, GateNext::RunChecks)
                {
                    self.unanswered(&gate.name, check, why);
                }
            }
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
            GateLine::Held { try_n, outcome, .. } => self.held_at(gate, *try_n, outcome),
            GateLine::Reopened { .. } | GateLine::NoneHeld { .. } => {}
        }
    }

    /// An `anytime` outcome held while the gate in hand waited for the agent or its checks.
    fn held_at(&mut self, gate: &Gate, try_n: u8, outcome: &OutcomeName) {
        let gates = self.gates;
        if self.waits(gate, try_n, GateNext::Agent) || self.waits(gate, try_n, GateNext::RunChecks)
        {
            self.held = gates
                .outcomes()
                .iter()
                .find(|o| o.anytime && o.name == *outcome);
        }
    }

    /// Takes in a line of the closing stage while it is open.
    fn decided(&mut self, line: &GateLine) {
        let gates = self.gates;
        match line {
            GateLine::Held { try_n, outcome, .. } if self.current(*try_n) => {
                self.held = gates.outcomes().iter().find(|o| o.name == *outcome);
            }
            GateLine::NoneHeld { try_n, .. } if self.current(*try_n) => {
                self.fail_by(CLOSING_TRIES, "no outcome held");
            }
            GateLine::NoAnswer {
                try_n, check, why, ..
            } if self.current(*try_n) => self.unanswered(&closing(), check, why),
            _ => {}
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
        } else {
            self.fail(gate, &format!("{} exited {exit}", named(gate, check)));
        }
    }

    /// moor got no answer from a check of stage `stage`; the second in a row waits for a person
    /// and spends no try, since it is not the agent's doing.
    fn unanswered(&mut self, stage: &StepId, check: &CheckOf, why: &Unanswered) {
        self.tries.unanswered += 1;
        if self.tries.unanswered >= 2 {
            let cause = self.gates.no_answer(stage, check, why);
            self.end = Some(End::Stuck {
                why: cause.why(),
                try_n: self.tries.used + 1,
                unanswered: Some(cause),
            });
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
            self.end = Some(End::Stuck {
                why: "the agent went quiet".to_owned(),
                try_n,
                unanswered: None,
            });
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

    /// The try in hand of `gate` failed.
    fn fail(&mut self, gate: &Gate, why: &str) {
        self.fail_by(gate.max_tries.get(), why);
    }

    /// The try in hand failed: the next one starts, or the `max` tries are used up.
    fn fail_by(&mut self, max: u8, why: &str) {
        self.tries.used += 1;
        if self.tries.used >= max {
            self.end = Some(End::Stuck {
                why: format!("out of tries: {why}"),
                try_n: self.tries.used,
                unanswered: None,
            });
        } else {
            self.tries.restart();
        }
    }
}
