//! What a Box's gates say to its agent: the preamble of its first message, the reminder for a
//! gate it has not reported, and the steer after a check failed.

use super::{Approval, Gate, Gates};
use crate::workflow::{Evidence, EvidenceName};

/// Where moor's preamble starts in a first message, after the person's own words.
const GATE_HEAD: &str = "\n\nmoor keeps this Box open until its gates pass.";

/// The person's own words of a first message [`Gates::first_message`] made; none for any other
/// message.
#[must_use]
pub fn split_gate_preamble(text: &str) -> Option<&str> {
    text.find(GATE_HEAD).map(|at| text.split_at(at).0)
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
             only when all have passed. report_step is a tool of the `moor` MCP server; if it is \
             a deferred tool, load it with ToolSearch first. Leave each evidence file in \
             .moor/artifacts/.\n\n{}",
            prompt.trim_end(),
            lines.join("\n")
        )
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
                 \"{name}\", result ok and those names.",
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
        let name = &self.name;
        format!(
            "Gate `{name}` has no report yet. Leave {} in .moor/artifacts/, then call \
             report_step with step \"{name}\", result ok and those names. Or call it with \
             result fail and a note that says why the gate cannot be passed.",
            names(self).join(", ")
        )
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
        if self.evidence.is_some() {
            text.push_str(&format!(
                "\n\nFix it, then call report_step again with step \"{name}\" and its evidence. \
                 moor goes through the gate again after your report."
            ));
        } else {
            text.push_str(
                "\n\nFix it, then stop. moor goes through the gate again once you are quiet.",
            );
        }
        text
    }

    /// The steer after check `check`, from 0, failed with `why` and printed `tail` at the end.
    /// A gate that asks for evidence needs a new report before its checks run again.
    #[must_use]
    pub fn failed_check_steer(&self, check: u8, (why, tail): (&str, &str)) -> String {
        let name = &self.name;
        let command = self
            .checks
            .get(usize::from(check))
            .map_or("", String::as_str);
        let mut text = format!(
            "Gate `{name}` is not passed: check {} (`{command}`) {why}.",
            check + 1
        );
        if !tail.trim().is_empty() {
            text.push_str("\n\nThe end of its output:\n");
            text.push_str(tail.trim_end());
        }
        text.push_str(&self.fix_it());
        text
    }
}
