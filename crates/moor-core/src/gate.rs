//! The gates a Box must pass before it counts as done.
//!
//! [`Gates::parse`] is the only way to build gates from text, so every gate in memory is valid.
//! [`status()`] reads where a Box stands from its gates and the lines written for them.

use std::collections::HashSet;

use schemars::JsonSchema;

use crate::workflow::{Evidence, EvidenceName, StepId, Tries, EVIDENCE_NAMES_MAX};

mod line;
mod status;
#[cfg(test)]
mod status_tests;
#[cfg(test)]
mod tests;

pub use line::GateLine;
pub use status::{due_check, status, DueCheck, GateNext, GateStatus};

/// The most gates a Box may have.
pub const GATES_MAX: usize = 8;

/// The most checks a gate may run.
pub const CHECKS_MAX: usize = 8;

/// The longest check command, in bytes.
pub const CHECK_MAX: usize = 4096;

/// Who must say yes before a gate passes.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Approval {
    /// Nobody.
    #[default]
    None,
    /// The Box's user.
    Owner,
}

/// One gate: what must hold before the next one opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gate {
    /// Its name, unique among the Box's gates.
    pub name: StepId,
    /// The files the agent must leave under `.moor/artifacts/`; none asks nothing of the agent.
    pub evidence: Option<Evidence>,
    /// Shell commands moor runs in the Box, in order; exit 0 passes.
    pub checks: Vec<String>,
    /// Who must approve after the checks.
    pub approval: Approval,
    /// How often the gate may be tried before it is stuck.
    pub max_tries: Tries,
}

/// One gate as written: the JSON a model sends and a Box keeps.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GateDoc {
    /// 1-32 characters of a-z, 0-9 and `-`, starting with a letter. Unique among the gates.
    pub name: String,
    /// Files the agent must leave under `.moor/artifacts/`: flat file names such as `report.md`,
    /// or a prefix ending in `*` such as `before-*`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<String>,
    /// Shell commands moor runs in the Box once the evidence is in, at most 8 of 4 KiB each. Exit 0
    /// passes; the first failure fails the try.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<String>,
    /// `owner` asks the Box's user to approve after the checks; `none` when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<Approval>,
    /// How often the gate may be tried, 1 to 5; 1 when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tries: Option<u64>,
}

/// Why gates are not valid.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GateError {
    /// The text is not gates: not JSON, a field of the wrong type, or an unknown field.
    #[error("the gates are not valid: {0}")]
    Json(String),
    /// No gates.
    #[error("a Box with gates needs at least one gate")]
    NoGates,
    /// More than [`GATES_MAX`].
    #[error("{0} gates; use at most 8")]
    TooManyGates(usize),
    /// A name that is not 1-32 of `[a-z0-9-]` starting with a letter.
    #[error("gate `{gate}`: use 1-32 lowercase letters, digits or '-', starting with a letter")]
    BadName {
        /// The name as written.
        gate: String,
    },
    /// Two gates share a name.
    #[error("gate name `{0}` is used twice")]
    DuplicateName(String),
    /// Nothing to pass: no evidence, no checks and no approval.
    #[error("gate `{gate}` needs evidence, a check or an owner approval")]
    NothingToPass {
        /// The gate.
        gate: String,
    },
    /// An evidence name that is neither an artifact name nor a `prefix*`.
    #[error("gate `{gate}`: evidence `{name}` is not a flat file name or a `prefix*`")]
    BadEvidence {
        /// The gate.
        gate: String,
        /// What was written.
        name: String,
    },
    /// More evidence names than a report may list.
    #[error("gate `{gate}` lists {len} evidence names; use at most 32")]
    TooManyEvidence {
        /// The gate.
        gate: String,
        /// How many.
        len: usize,
    },
    /// More than [`CHECKS_MAX`] checks.
    #[error("gate `{gate}` has {len} checks; use at most 8")]
    TooManyChecks {
        /// The gate.
        gate: String,
        /// How many.
        len: usize,
    },
    /// A check with no command.
    #[error("gate `{gate}` has an empty check")]
    EmptyCheck {
        /// The gate.
        gate: String,
    },
    /// A check over [`CHECK_MAX`] bytes.
    #[error("gate `{gate}`: a check is {len} bytes; use at most 4096")]
    CheckTooLong {
        /// The gate.
        gate: String,
        /// Its length.
        len: usize,
    },
    /// A `max_tries` outside 1 to 5.
    #[error("gate `{gate}`: max_tries must be 1 to 5, not {got}")]
    MaxTries {
        /// The gate.
        gate: String,
        /// What was written.
        got: u64,
    },
}

/// A Box's gates: one to eight, in the order they pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gates(Vec<Gate>);

impl Gates {
    /// The gates `docs` describe.
    ///
    /// # Errors
    /// [`GateError`] for the first thing wrong.
    pub fn parse(docs: &[GateDoc]) -> Result<Self, GateError> {
        if docs.is_empty() {
            return Err(GateError::NoGates);
        }
        if docs.len() > GATES_MAX {
            return Err(GateError::TooManyGates(docs.len()));
        }
        let mut seen = HashSet::new();
        let mut gates = Vec::with_capacity(docs.len());
        for doc in docs {
            let gate = parse_gate(doc)?;
            if !seen.insert(gate.name.clone()) {
                return Err(GateError::DuplicateName(doc.name.clone()));
            }
            gates.push(gate);
        }
        Ok(Self(gates))
    }

    /// The gates in `text`, a JSON array of [`GateDoc`].
    ///
    /// # Errors
    /// [`GateError`] when it is not JSON of that shape, or its gates are not valid.
    pub fn from_json(text: &str) -> Result<Self, GateError> {
        let docs: Vec<GateDoc> =
            serde_json::from_str(text).map_err(|e| GateError::Json(e.to_string()))?;
        Self::parse(&docs)
    }

    /// The gates as written, leaving out what is default.
    #[must_use]
    pub fn docs(&self) -> Vec<GateDoc> {
        self.0.iter().map(doc_of).collect()
    }

    /// Every gate, the first one first.
    pub fn iter(&self) -> impl Iterator<Item = &Gate> {
        self.0.iter()
    }

    /// The gate at place `at`, from 0.
    #[must_use]
    pub fn get(&self, at: usize) -> Option<&Gate> {
        self.0.get(at)
    }
}

/// The gate `doc` describes.
fn parse_gate(doc: &GateDoc) -> Result<Gate, GateError> {
    let gate = || doc.name.clone();
    let name = StepId::parse(&doc.name).map_err(|_| GateError::BadName { gate: gate() })?;
    let evidence = parse_evidence(doc)?;
    parse_checks(doc)?;
    let approval = doc.approval.unwrap_or_default();
    if evidence.is_none() && doc.checks.is_empty() && approval == Approval::None {
        return Err(GateError::NothingToPass { gate: gate() });
    }
    let max_tries = match doc.max_tries {
        None => Tries::ONE,
        Some(n) => {
            u8::try_from(n)
                .ok()
                .and_then(Tries::new)
                .ok_or_else(|| GateError::MaxTries {
                    gate: gate(),
                    got: n,
                })?
        }
    };
    Ok(Gate {
        name,
        evidence,
        checks: doc.checks.clone(),
        approval,
        max_tries,
    })
}

/// The evidence `doc` lists; none when it lists none.
fn parse_evidence(doc: &GateDoc) -> Result<Option<Evidence>, GateError> {
    if doc.evidence.len() > EVIDENCE_NAMES_MAX {
        return Err(GateError::TooManyEvidence {
            gate: doc.name.clone(),
            len: doc.evidence.len(),
        });
    }
    let names = doc
        .evidence
        .iter()
        .map(|n| {
            EvidenceName::parse(n).ok_or_else(|| GateError::BadEvidence {
                gate: doc.name.clone(),
                name: n.clone(),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Evidence::new(names))
}

/// Refuses checks that are too many, empty or too long.
fn parse_checks(doc: &GateDoc) -> Result<(), GateError> {
    let gate = || doc.name.clone();
    if doc.checks.len() > CHECKS_MAX {
        return Err(GateError::TooManyChecks {
            gate: gate(),
            len: doc.checks.len(),
        });
    }
    for check in &doc.checks {
        if check.trim().is_empty() {
            return Err(GateError::EmptyCheck { gate: gate() });
        }
        if check.len() > CHECK_MAX {
            return Err(GateError::CheckTooLong {
                gate: gate(),
                len: check.len(),
            });
        }
    }
    Ok(())
}

/// `gate` as written.
fn doc_of(gate: &Gate) -> GateDoc {
    GateDoc {
        name: gate.name.to_string(),
        evidence: gate
            .evidence
            .iter()
            .flat_map(Evidence::names)
            .map(|n| n.as_str().to_owned())
            .collect(),
        checks: gate.checks.clone(),
        approval: (gate.approval != Approval::None).then_some(gate.approval),
        max_tries: (gate.max_tries != Tries::ONE).then(|| u64::from(gate.max_tries.get())),
    }
}
