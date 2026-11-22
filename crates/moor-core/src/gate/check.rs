//! A gate's check: the command moor runs, and the plain sentence it stands for when its author wrote one.

use schemars::JsonSchema;

use super::{CheckOf, Gate, GateError, Gates, Unanswered};
use crate::workflow::StepId;

/// The longest sentence a check may say, in characters.
pub const SAYS_MAX: usize = 200;

/// One check as written: a bare command, or a command with what must be true.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum CheckDoc {
    /// A command moor runs in the Box; exit 0 passes.
    Run(String),
    /// A command and the one plain sentence that must be true when it passes.
    Said(SaidDoc),
}

/// A check with its sentence.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SaidDoc {
    /// The shell command moor runs in the Box, up to 4 KiB. Exit 0 passes.
    pub run: String,
    /// One plain sentence of what must be true, up to 200 characters, such as "The pull request
    /// has one evidence comment." A person reads it where the command would be, before it.
    pub says: String,
}

impl From<&str> for CheckDoc {
    fn from(run: &str) -> Self {
        Self::Run(run.to_owned())
    }
}

impl From<String> for CheckDoc {
    fn from(run: String) -> Self {
        Self::Run(run)
    }
}

/// A check that parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    /// The shell command moor runs in the Box.
    pub run: String,
    /// What must be true, in one plain sentence; none for a bare command.
    pub says: Option<String>,
}

impl Check {
    /// The check `doc` describes, for gate `gate`.
    pub(super) fn parse(doc: &CheckDoc, gate: &str) -> Result<Self, GateError> {
        let (run, says) = match doc {
            CheckDoc::Run(run) => (run, None),
            CheckDoc::Said(d) => (&d.run, Some(d.says.trim())),
        };
        let gate = || gate.to_owned();
        if run.trim().is_empty() {
            return Err(GateError::EmptyCheck { gate: gate() });
        }
        if run.len() > super::CHECK_MAX {
            return Err(GateError::CheckTooLong {
                gate: gate(),
                len: run.len(),
            });
        }
        let says = says.map(|s| said(s, &gate())).transpose()?;
        Ok(Self {
            run: run.clone(),
            says,
        })
    }

    /// The check as written: a bare command when it says nothing.
    pub(super) fn doc(&self) -> CheckDoc {
        self.says.as_ref().map_or_else(
            || CheckDoc::Run(self.run.clone()),
            |says| {
                CheckDoc::Said(SaidDoc {
                    run: self.run.clone(),
                    says: says.clone(),
                })
            },
        )
    }
}

impl Gate {
    /// Check `check`, from 0, as a person reads it: what it says, or its command cut short and
    /// in backticks when it says nothing.
    #[must_use]
    pub fn check_label(&self, check: u8) -> String {
        let Some(c) = self.checks.get(usize::from(check)) else {
            return "``".to_owned();
        };
        c.says.clone().unwrap_or_else(|| cut(&c.run))
    }
}

/// Command `run` cut short and in backticks.
fn cut(run: &str) -> String {
    let run = run.trim();
    let cut: String = run.chars().take(60).collect();
    let more = if run.chars().count() > 60 { "…" } else { "" };
    format!("`{cut}{more}`")
}

impl Gates {
    /// The check `of` names at stage `stage`: a gate's own, or an outcome's.
    #[must_use]
    pub fn check_of(&self, stage: &StepId, of: &CheckOf) -> Option<&Check> {
        match of {
            CheckOf::Gate { check } => self
                .iter()
                .find(|g| g.name == *stage)?
                .checks
                .get(usize::from(*check)),
            CheckOf::Outcome { outcome, check } => self
                .outcomes()
                .iter()
                .find(|o| o.name == *outcome)?
                .checks
                .get(usize::from(*check)),
        }
    }

    /// Why stage `stage` is stuck after moor got no answer from check `of` twice.
    pub(super) fn unanswered_why(&self, stage: &StepId, of: &CheckOf, why: &Unanswered) -> String {
        let (whose, check) = match of {
            CheckOf::Gate { check } => (String::new(), *check),
            CheckOf::Outcome { outcome, check } => (format!("outcome `{outcome}` "), *check),
        };
        let what = match self.check_of(stage, of) {
            Some(Check {
                says: Some(says), ..
            }) => format!(" (“{says}”)"),
            Some(Check { run, says: None }) => format!(" ({})", cut(run)),
            None => String::new(),
        };
        let reason = match why {
            Unanswered::NotRun { reason } => reason.trim(),
            Unanswered::Lost => "its output stream was lost",
        };
        format!(
            "moor, not the agent, could not run {whose}check {}{what} twice: {reason}",
            check + 1
        )
    }
}

/// `says`, trimmed, when it is one line of 1 to [`SAYS_MAX`] characters; errors name `gate`.
pub(super) fn said(says: &str, gate: &str) -> Result<String, GateError> {
    let gate = || gate.to_owned();
    if says.is_empty() {
        return Err(GateError::EmptySays { gate: gate() });
    }
    if says.contains('\n') {
        return Err(GateError::SaysNotOneLine { gate: gate() });
    }
    let len = says.chars().count();
    if len > SAYS_MAX {
        return Err(GateError::SaysTooLong { gate: gate(), len });
    }
    Ok(says.to_owned())
}

/// Check `check`, from 0, of `gate` as a person reads it: its place, then its label.
pub(super) fn named(gate: &Gate, check: u8) -> String {
    let place = check + 1;
    let label = gate.check_label(check);
    match gate
        .checks
        .get(usize::from(check))
        .and_then(|c| c.says.as_ref())
    {
        Some(_) => format!("check {place} (“{label}”)"),
        None => format!("check {place} ({label})"),
    }
}
