//! The gates a Box must pass before it counts as done.
//!
//! [`Gates::parse`] is the only way to build gates from text, so every gate in memory is valid.
//! [`status()`] reads where a Box stands from its gates and the lines written for them.

use std::collections::HashSet;

use schemars::JsonSchema;

use crate::workflow::{Evidence, EvidenceName, StepId, Tries, EVIDENCE_NAMES_MAX};

mod brief;
#[cfg(test)]
mod brief_tests;
mod check;
#[cfg(test)]
mod check_tests;
mod error;
mod fold;
mod line;
mod outcome;
#[cfg(test)]
mod outcome_tests;
mod shown;
mod status;
#[cfg(test)]
mod status_tests;
#[cfg(test)]
mod tests;
mod wait;

pub use brief::{split_gate_preamble, split_owner_note};
pub use check::{Check, CheckDoc, SaidDoc, SAYS_MAX};
pub use error::GateError;
pub use line::{GateLine, Tried};
pub use outcome::{
    closing, Outcome, OutcomeDoc, OutcomeName, Tone, Verdict, CLOSING, OUTCOMES_MAX,
    OUTCOME_NAME_MAX,
};
pub use status::{
    due_check, due_outcomes, passed_gates, status, DueCheck, DueOutcomes, GateNext, GateStatus,
};
pub use wait::{wait, Wait, WaitKind};

/// The most gates a Box may have.
pub const GATES_MAX: usize = 8;

/// The most checks a gate may run.
pub const CHECKS_MAX: usize = 8;

/// The longest check command, in bytes.
pub const CHECK_MAX: usize = 4096;

/// How often a gate may be tried when its `max_tries` is left out.
const DEFAULT_TRIES: Tries = Tries::TWO;

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
    /// What moor runs in the Box, in order; exit 0 passes.
    pub checks: Vec<Check>,
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
    /// What moor runs in the Box once the evidence is in, at most 8. Each is a shell command of up
    /// to 4 KiB, or `{run, says}` where `says` is one plain sentence of what must be true. Exit 0
    /// passes; the first failure fails the try.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<CheckDoc>,
    /// `owner` asks the Box's user to approve after the checks; `none` when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<Approval>,
    /// How often the gate may be tried, 1 to 5; 2 when left out, so a failed check or an owner's
    /// no leaves the agent a chance to fix it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tries: Option<u64>,
}

/// Gates and outcomes as a Box or a workflow keeps them.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GatesDoc {
    /// The gates.
    pub gates: Vec<GateDoc>,
    /// The outcomes; none for the one implicit outcome.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outcomes: Vec<OutcomeDoc>,
}

/// A Box's gates, one to eight in the order they pass, and the outcomes its run may end in, at
/// least one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gates {
    /// The gates.
    gates: Vec<Gate>,
    /// The outcomes, in the order moor tries them.
    outcomes: Vec<Outcome>,
}

impl Gates {
    /// The gates `docs` and the outcomes `outcomes` describe; no outcomes gives the one
    /// implicit outcome `done`, which holds once every gate has passed.
    ///
    /// # Errors
    /// [`GateError`] for the first thing wrong.
    pub fn parse(docs: &[GateDoc], outcomes: &[OutcomeDoc]) -> Result<Self, GateError> {
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
        let outcomes = outcome::parse_outcomes(outcomes)?;
        Ok(Self { gates, outcomes })
    }

    /// The gates in `text`, the JSON of a [`GatesDoc`].
    ///
    /// # Errors
    /// [`GateError`] when it is not JSON of that shape, or its gates are not valid.
    pub fn from_json(text: &str) -> Result<Self, GateError> {
        let doc: GatesDoc =
            serde_json::from_str(text).map_err(|e| GateError::Json(e.to_string()))?;
        Self::parse(&doc.gates, &doc.outcomes)
    }

    /// The gates and outcomes as a Box or a workflow keeps them.
    #[must_use]
    pub fn doc(&self) -> GatesDoc {
        GatesDoc {
            gates: self.docs(),
            outcomes: self.outcome_docs(),
        }
    }

    /// The gates as written, leaving out what is default.
    #[must_use]
    pub fn docs(&self) -> Vec<GateDoc> {
        self.gates.iter().map(doc_of).collect()
    }

    /// The outcomes as written; none when there is only the implicit one.
    #[must_use]
    pub fn outcome_docs(&self) -> Vec<OutcomeDoc> {
        if self.outcomes == [Outcome::implicit()] {
            return Vec::new();
        }
        self.outcomes.iter().map(Outcome::doc).collect()
    }

    /// The outcomes, in the order moor tries them.
    #[must_use]
    pub fn outcomes(&self) -> &[Outcome] {
        &self.outcomes
    }

    /// Whether a gate needs the agent to call `report_step` or the owner to answer, which only a
    /// controller that offers `report_step` can honour.
    #[must_use]
    pub fn needs_reports(&self) -> bool {
        self.iter()
            .any(|g| g.evidence.is_some() || g.approval != Approval::None)
    }

    /// Every gate, the first one first.
    pub fn iter(&self) -> impl Iterator<Item = &Gate> {
        self.gates.iter()
    }

    /// The gate at place `at`, from 0.
    #[must_use]
    pub fn get(&self, at: usize) -> Option<&Gate> {
        self.gates.get(at)
    }
}

/// The gate `doc` describes.
fn parse_gate(doc: &GateDoc) -> Result<Gate, GateError> {
    let gate = || doc.name.clone();
    let name = StepId::parse(&doc.name).map_err(|_| GateError::BadName { gate: gate() })?;
    if name.as_str() == CLOSING {
        return Err(GateError::ReservedName(gate()));
    }
    let evidence = parse_evidence(doc)?;
    let checks = parse_checks(doc)?;
    let approval = doc.approval.unwrap_or_default();
    if evidence.is_none() && checks.is_empty() && approval == Approval::None {
        return Err(GateError::NothingToPass { gate: gate() });
    }
    let max_tries = match doc.max_tries {
        None => DEFAULT_TRIES,
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
        checks,
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

/// The checks `doc` lists, refusing too many, empty or too long ones.
fn parse_checks(doc: &GateDoc) -> Result<Vec<Check>, GateError> {
    if doc.checks.len() > CHECKS_MAX {
        return Err(GateError::TooManyChecks {
            gate: doc.name.clone(),
            len: doc.checks.len(),
        });
    }
    doc.checks
        .iter()
        .map(|c| Check::parse(c, &doc.name))
        .collect()
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
        checks: gate.checks.iter().map(Check::doc).collect(),
        approval: (gate.approval != Approval::None).then_some(gate.approval),
        max_tries: (gate.max_tries != DEFAULT_TRIES).then(|| u64::from(gate.max_tries.get())),
    }
}
