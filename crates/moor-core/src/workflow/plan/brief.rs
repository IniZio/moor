//! What a plan says to the agent and to the owner: the first prompt, the steer for each later
//! step, and the numbered list the owner reviews.

use super::{Plan, Step, StepKind};

impl Step {
    /// The step as the agent is told it.
    #[must_use]
    pub fn brief(&self) -> String {
        let head = format!("Current step: {}, \"{}\"", self.id, self.title);
        match &self.kind {
            StepKind::Work {
                instructions,
                done_when,
                evidence,
                ..
            } => {
                let names: Vec<&str> = evidence.names().map(super::EvidenceName::as_str).collect();
                format!(
                    "{head}\n{instructions}\nDone when: {done_when}\nEvidence: {}",
                    names.join(", ")
                )
            }
            StepKind::Deliver {
                instructions,
                done_when,
            } => format!("{head}\n{instructions}\nDone when: {done_when}"),
            StepKind::Check { command, .. } => format!("{head}\nmoor runs: {command}"),
            StepKind::Approve { question, .. } => {
                format!("{head}\nmoor asks the owner: {question}")
            }
        }
    }

    /// The step as one line of the list the owner reviews.
    #[must_use]
    pub fn line(&self) -> String {
        let route = self.kind.next().map_or_else(
            || "ends the run".to_owned(),
            |n| {
                n.fail.as_ref().map_or_else(
                    || format!("ok: {}", n.ok),
                    |f| format!("ok: {}, fail: {f}", n.ok),
                )
            },
        );
        let tries = match self.max_tries.get() {
            1 => String::new(),
            n => format!(", up to {n} tries"),
        };
        format!(
            "{} ({}): {} [{route}{tries}]",
            self.id,
            self.kind.name(),
            self.title
        )
    }
}

impl Plan {
    /// What a Box's agent is first asked: `prompt`, then the preamble and the first step.
    #[must_use]
    pub fn start_prompt(&self, prompt: &str) -> String {
        format!(
            "{prompt}\n\nmoor runs this as a plan of {} steps. Do only the current step, then \
             stop: moor sends the next step when you stop. Leave each evidence file in \
             .moor/artifacts/.\n\n{}",
            self.step_count(),
            self.first().brief()
        )
    }

    /// The steer that sends `step` after the step before it is done.
    #[must_use]
    pub fn next_step_steer(step: &Step) -> String {
        format!(
            "That step is done. Do only the next step, then stop: moor sends the one after \
             when you stop.\n\n{}",
            step.brief()
        )
    }

    /// The steps as a numbered list, one line each.
    #[must_use]
    pub fn outline(&self) -> Vec<String> {
        self.steps()
            .enumerate()
            .map(|(i, s)| format!("{}. {}", i + 1, s.line()))
            .collect()
    }
}
