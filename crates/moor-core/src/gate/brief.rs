//! What a Box's gates say to its agent: the preamble of its first message, the reminder for a
//! gate it has not reported, and the steer after a check failed.

use super::{Approval, Check, Gate, Gates, Outcome, Tried};
use crate::workflow::{Evidence, EvidenceName, StepId};

/// Where moor's preamble starts in a first message, after the person's own words.
const GATE_HEAD: &str = "\n\nmoor keeps this Box open until its gates pass.";

/// The person's own words of a first message [`Gates::first_message`] made; none for any other
/// message.
#[must_use]
pub fn split_gate_preamble(text: &str) -> Option<&str> {
    text.find(GATE_HEAD).map(|at| text.split_at(at).0)
}

/// How moor's steer after a refusal introduces what the owner wrote.
const OWNER_WROTE: &str = "\n\nThe owner wrote: ";

/// Where what the agent does next starts in that steer, after the owner's words.
const TRY_AGAIN: &str = "\n\nFix it, then";

/// The owner's own words in a steer [`Gate::owner_no_steer`] made, and the rest of the steer
/// without them; none for any other message.
#[must_use]
pub fn split_owner_note(text: &str) -> Option<(String, &str)> {
    if !text.starts_with("The owner did not approve gate ") {
        return None;
    }
    let (head, rest) = text.split_once(OWNER_WROTE)?;
    let (note, tail) = rest.rsplit_once(TRY_AGAIN)?;
    Some((format!("{head}{TRY_AGAIN}{tail}"), note))
}

/// The evidence names of `gate`, as the agent reads them.
fn names(gate: &Gate) -> Vec<&str> {
    gate.evidence
        .iter()
        .flat_map(Evidence::names)
        .map(EvidenceName::as_str)
        .collect()
}

impl Gates {
    /// What a Box's agent is first asked: `prompt`, then what its gates ask of it.
    #[must_use]
    pub fn first_message(&self, prompt: &str) -> String {
        let lines: Vec<String> = self.iter().map(Gate::brief).collect();
        format!(
            "{}{GATE_HEAD} The gates open in this order, one at a time, and the Box is done \
             only when all have passed. The first gate is in hand now.\n\n{}{}",
            prompt.trim_end(),
            lines.join("\n"),
            self.outcomes_brief()
        )
    }

    /// What the preamble says of the outcomes; nothing when there is only the implicit one.
    fn outcomes_brief(&self) -> String {
        if self.outcome_docs().is_empty() {
            return String::new();
        }
        let lines: Vec<String> = self.outcomes().iter().map(Outcome::brief).collect();
        format!(
            "\n\nThe run's outcomes, in the order moor tries them: the first whose checks all \
             pass is how the run ends. moor tries them once every gate has passed, and tries \
             those marked \"anytime\" also whenever you stop before the gate in hand has \
             passed.\n\n{}",
            lines.join("\n")
        )
    }

    /// Each outcome in `tried` with the check that stopped it and the end of its output.
    #[must_use]
    pub fn tried_text(&self, tried: &[Tried]) -> String {
        let lines: Vec<String> = tried
            .iter()
            .map(|t| {
                let check = self
                    .outcomes()
                    .iter()
                    .find(|o| o.name == t.outcome)
                    .and_then(|o| o.checks.get(usize::from(t.check)));
                let what = match check {
                    Some(Check {
                        says: Some(says),
                        run,
                    }) => format!("“{says}” (`{run}`)"),
                    Some(Check { run, says: None }) => format!("`{run}`"),
                    None => format!("check {}", t.check + 1),
                };
                let mut line = format!("- `{}`: {what} exited {}.", t.outcome, t.exit);
                if !t.tail.trim().is_empty() {
                    line.push_str(" The end of its output:\n");
                    line.push_str(t.tail.trim_end());
                }
                line
            })
            .collect();
        lines.join("\n")
    }

    /// The steer after moor tried the outcomes at the closing stage and none held.
    #[must_use]
    pub fn none_held_steer(&self, tried: &[Tried]) -> String {
        format!(
            "Every gate has passed, but no outcome holds yet:\n{}\n\nFix what is missing, then \
             stop. moor tries the outcomes again once you are quiet.",
            self.tried_text(tried)
        )
    }
}

impl Outcome {
    /// The outcome as one line of the preamble.
    fn brief(&self) -> String {
        let mut line = format!("- `{}`", self.name);
        if let Some(says) = &self.says {
            line.push_str(": ");
            line.push_str(says);
        }
        if self.anytime {
            line.push_str(" (anytime)");
        }
        let said: Vec<String> = self
            .checks
            .iter()
            .filter_map(|c| c.says.as_deref())
            .map(|s| format!("“{s}”"))
            .collect();
        if !said.is_empty() {
            line.push_str(" It holds when: ");
            line.push_str(&said.join(" "));
        }
        line
    }
}

impl Gate {
    /// The gate as one line of the preamble: what the agent hands in for it, and what moor does.
    #[must_use]
    pub fn brief(&self) -> String {
        let name = &self.name;
        let mut line = if self.evidence.is_some() {
            format!(
                "- Gate `{name}`: leave {} in .moor/artifacts/, then call report_step with step \
                 \"{name}\", result ok, those names and a one-line note of what you did.",
                names(self).join(", ")
            )
        } else {
            format!("- Gate `{name}`: you report nothing for it.")
        };
        let checks = match self.checks.len() {
            0 => None,
            1 => Some("its check".to_owned()),
            n => Some(format!("its {n} checks")),
        };
        if let Some(checks) = checks {
            line.push_str(" moor runs ");
            line.push_str(&checks);
            line.push_str(" itself once you are quiet.");
            let said: Vec<String> = self
                .checks
                .iter()
                .filter_map(|c| c.says.as_deref())
                .map(|s| format!("“{s}”"))
                .collect();
            if !said.is_empty() {
                line.push_str(" It looks for: ");
                line.push_str(&said.join(" "));
                line.push('.');
            }
        }
        if self.approval == Approval::Owner {
            line.push_str(" Then moor asks the Box's owner to approve.");
        }
        line
    }

    /// What a failed check asks of the agent, which depends on whether the gate takes a report.
    fn fix_it(&self) -> String {
        if self.evidence.is_some() {
            format!(
                "\n\nFix it, then call report_step again with step \"{}\" and its evidence. moor \
                 runs the checks again after your report.",
                self.name
            )
        } else {
            "\n\nFix it, then stop. moor runs the checks again once you are quiet.".to_owned()
        }
    }

    /// The steer for a gate whose evidence the agent has not reported. One per try.
    #[must_use]
    pub fn remind_steer(&self) -> String {
        format!(
            "Gate `{}` has no report yet.\n\n{}",
            self.name,
            self.brief()
        )
    }

    /// The steer when gate `passed` has passed and this gate, which waits for the agent, is now in
    /// hand. It is not the reminder.
    #[must_use]
    pub fn handoff_steer(&self, passed: &StepId) -> String {
        format!(
            "Gate `{passed}` has passed. Gate `{}` is now in hand.\n\n{}",
            self.name,
            self.brief()
        )
    }

    /// What the agent does after a try the owner refused, before moor goes through the gate again.
    fn try_again(&self) -> String {
        let name = &self.name;
        if self.evidence.is_some() {
            format!(
                "\n\nFix it, then call report_step again with step \"{name}\" and its evidence. \
                 moor goes through the gate again after your report."
            )
        } else {
            "\n\nFix it, then stop. moor goes through the gate again once you are quiet.".to_owned()
        }
    }

    /// The steer after the owner said no to a try that leaves more tries: what they wrote, then
    /// what the agent does next.
    #[must_use]
    pub fn owner_no_steer(&self, note: &str) -> String {
        let name = &self.name;
        let mut text = format!("The owner did not approve gate `{name}`.");
        if !note.trim().is_empty() {
            text.push_str("\n\nThe owner wrote: ");
            text.push_str(note.trim());
        }
        text.push_str(&self.try_again());
        text
    }

    /// The steer after check `check`, from 0, failed with `why` and printed `tail` at the end.
    /// A gate that asks for evidence needs a new report before its checks run again.
    #[must_use]
    pub fn failed_check_steer(&self, check: u8, (why, tail): (&str, &str)) -> String {
        let name = &self.name;
        let place = check + 1;
        let mut text = match self.checks.get(usize::from(check)) {
            Some(Check {
                run,
                says: Some(says),
            }) => {
                format!("Gate `{name}` is not passed: check {place}, “{says}” (`{run}`), {why}.")
            }
            Some(Check { run, says: None }) => {
                format!("Gate `{name}` is not passed: check {place} (`{run}`) {why}.")
            }
            None => format!("Gate `{name}` is not passed: check {place} {why}."),
        };
        if !tail.trim().is_empty() {
            text.push_str("\n\nThe end of its output:\n");
            text.push_str(tail.trim_end());
        }
        text.push_str(&self.fix_it());
        text
    }
}
