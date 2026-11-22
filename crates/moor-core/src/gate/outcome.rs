//! How a run with gates ends: the outcomes its definition names, which moor decides by running
//! their checks. The agent never claims one.

use schemars::JsonSchema;

use super::check::said;
use super::{Check, CheckDoc, GateError, CHECKS_MAX};
use crate::workflow::StepId;

/// The most outcomes a definition may name.
pub const OUTCOMES_MAX: usize = 6;

/// The longest outcome name, in characters.
pub const OUTCOME_NAME_MAX: usize = 40;

/// The stage after the last gate, where moor decides the outcome. No gate may take its name.
pub const CLOSING: &str = "outcome";

/// The name of the one outcome of a definition that names none.
const IMPLICIT: &str = "done";

/// How the closing stage may be tried before it is stuck.
pub(super) const CLOSING_TRIES: u8 = 2;

/// How a person reads an outcome.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Tone {
    /// What the run was for came true.
    #[default]
    Good,
    /// The run did its job, and what it found is bad news, such as a failed check.
    Bad,
    /// Neither, such as nothing to do.
    Plain,
}

/// An outcome's name: one short line, unique among a definition's outcomes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct OutcomeName(pub(super) String);

impl OutcomeName {
    /// The name as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for OutcomeName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One outcome a run may end in. At the closing stage moor tries every outcome, `anytime` ones
/// included, in the order listed, and the first that holds is the run's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// Its name.
    pub name: OutcomeName,
    /// What it means, in one sentence.
    pub says: Option<String>,
    /// How a person reads it.
    pub tone: Tone,
    /// What moor runs in the Box; the outcome holds when all exit 0, and one with none always
    /// holds.
    pub checks: Vec<Check>,
    /// Whether moor also tries it when the agent goes quiet before the gate in hand has passed.
    pub anytime: bool,
}

/// One outcome as written.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OutcomeDoc {
    /// One line of up to 40 characters, such as "check passed". Unique among the outcomes.
    pub name: String,
    /// One plain sentence of what the outcome means, up to 200 characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub says: Option<String>,
    /// `good` when left out, `bad` or `plain`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tone: Option<Tone>,
    /// What moor runs in the Box, at most 8, as a gate's checks are written. The outcome holds
    /// when all exit 0; moor tries the outcomes in the order listed and the first that holds is
    /// the run's.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<CheckDoc>,
    /// True to also try it whenever the agent goes quiet before the gate in hand has passed;
    /// when it holds the run ends at once. Needs a check.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub anytime: bool,
}

/// What a run ended in, as its views and history keep it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, JsonSchema)]
pub struct Verdict {
    /// The outcome's name.
    pub name: OutcomeName,
    /// How a person reads it.
    pub tone: Tone,
    /// What it means, when its author wrote it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub says: Option<String>,
}

impl Outcome {
    /// The outcome as a run keeps it.
    #[must_use]
    pub fn verdict(&self) -> Verdict {
        Verdict {
            name: self.name.clone(),
            tone: self.tone,
            says: self.says.clone(),
        }
    }

    /// The outcome as written.
    pub(super) fn doc(&self) -> OutcomeDoc {
        OutcomeDoc {
            name: self.name.0.clone(),
            says: self.says.clone(),
            tone: (self.tone != Tone::Good).then_some(self.tone),
            checks: self.checks.iter().map(Check::doc).collect(),
            anytime: self.anytime,
        }
    }

    /// The one outcome of a definition that names none: it always holds.
    pub(super) fn implicit() -> Self {
        Self {
            name: OutcomeName(IMPLICIT.to_owned()),
            says: None,
            tone: Tone::Good,
            checks: Vec::new(),
            anytime: false,
        }
    }
}

/// The stage after the last gate, as a step id.
#[must_use]
pub fn closing() -> StepId {
    StepId::reserved(CLOSING)
}

/// The outcomes `docs` describe; the implicit one when there are none.
pub(super) fn parse_outcomes(docs: &[OutcomeDoc]) -> Result<Vec<Outcome>, GateError> {
    if docs.is_empty() {
        return Ok(vec![Outcome::implicit()]);
    }
    if docs.len() > OUTCOMES_MAX {
        return Err(GateError::TooManyOutcomes(docs.len()));
    }
    let outcomes = docs
        .iter()
        .map(parse_outcome)
        .collect::<Result<Vec<_>, _>>()?;
    let mut seen = std::collections::HashSet::new();
    if let Some(o) = outcomes.iter().find(|o| !seen.insert(&o.name)) {
        return Err(GateError::DuplicateOutcome(o.name.0.clone()));
    }
    let settled: Vec<&Outcome> = outcomes.iter().filter(|o| !o.anytime).collect();
    let Some((_, before)) = settled.split_last() else {
        return Err(GateError::OnlyAnytime);
    };
    if let Some(o) = before.iter().find(|o| o.checks.is_empty()) {
        return Err(GateError::OtherwiseNotLast(o.name.0.clone()));
    }
    Ok(outcomes)
}

/// The outcome `doc` describes.
fn parse_outcome(doc: &OutcomeDoc) -> Result<Outcome, GateError> {
    let name = doc.name.trim();
    if name.is_empty() || name.contains('\n') || name.chars().count() > OUTCOME_NAME_MAX {
        return Err(GateError::BadOutcomeName(doc.name.clone()));
    }
    let bad = |e: GateError| GateError::BadOutcome {
        outcome: name.to_owned(),
        why: e.to_string(),
    };
    let says = doc
        .says
        .as_deref()
        .map(|s| said(s.trim(), name))
        .transpose()
        .map_err(bad)?;
    if doc.checks.len() > CHECKS_MAX {
        return Err(GateError::TooManyChecks {
            gate: name.to_owned(),
            len: doc.checks.len(),
        });
    }
    let checks = doc
        .checks
        .iter()
        .map(|c| Check::parse(c, name))
        .collect::<Result<Vec<_>, _>>()
        .map_err(bad)?;
    if doc.anytime && checks.is_empty() {
        return Err(GateError::AnytimeWithoutCheck(name.to_owned()));
    }
    Ok(Outcome {
        name: OutcomeName(name.to_owned()),
        says,
        tone: doc.tone.unwrap_or_default(),
        checks,
        anytime: doc.anytime,
    })
}
