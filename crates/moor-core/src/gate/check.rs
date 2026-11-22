//! A gate's check: the command moor runs, and the plain sentence it stands for when its author wrote one.

use schemars::JsonSchema;

use super::{Gate, GateError};

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
        if let Some(says) = says {
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
        }
        Ok(Self {
            run: run.clone(),
            says: says.map(str::to_owned),
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
        if let Some(says) = &c.says {
            return says.clone();
        }
        let run = c.run.trim();
        let cut: String = run.chars().take(60).collect();
        let more = if run.chars().count() > 60 { "…" } else { "" };
        format!("`{cut}{more}`")
    }
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
