//! What a Box's gates say to its agent: the preamble of its first message, the reminder for a
//! gate it has not reported, and the steer after a check failed.

use super::{Approval, Gate, Gates};
use crate::workflow::EvidenceName;

/// Where moor's preamble starts in a first message, after the person's own words.
const GATE_HEAD: &str = "\n\nmoor holds this task open until its gates pass.";

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
        .flat_map(|e| e.names())
        .map(EvidenceName::as_str)
        .collect()
}

impl Gates {
    /// What a Box's agent is first asked: `prompt`, then what its gates ask of it.
    #[must_use]
    pub fn first_message(&self, prompt: &str) -> String {
        let lines: Vec<String> = self.iter().map(Gate::line).collect();
        format!(
            "{}{GATE_HEAD} The gates open in this order, one at a time, and the task is done \
             only when all have passed. report_step is a tool of the `moor` MCP server; if it is \
             a deferred tool, load it with ToolSearch first. Leave each evidence file in \
             .moor/artifacts/.\n\n{}",
            prompt.trim_end(),
            lines.join("\n")
        )
    }
}

impl Gate {
    /// The gate as one line of the preamble.
    fn line(&self) -> String {
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
        match self.checks.len() {
            0 => {}
            1 => line.push_str(" moor runs its check itself once you are quiet."),
            n => line.push_str(&format!(
                " moor runs its {n} checks itself once you are quiet."
            )),
        }
        if self.approval == Approval::Owner {
            line.push_str(" Then moor asks the Box's owner to approve.");
        }
        line
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
        if self.evidence.is_some() {
            text.push_str(&format!(
                "\n\nFix it, then call report_step again with step \"{name}\" and its \
                 evidence. moor runs the checks again after your report."
            ));
        } else {
            text.push_str("\n\nFix it, then stop. moor runs the checks again once you are quiet.");
        }
        text
    }
}
