//! What a read holds the agent to, the key that matches an effect across reads, and how a read
//! differs from the one held.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use super::{EffectId, Read};
use crate::gate::{GateDoc, OutcomeDoc};

/// What moor holds. Each one is checked when the read is handed in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Effect {
    /// A gate the run's Box must pass.
    Gate {
        /// Its id.
        id: EffectId,
        /// The gate as written.
        doc: GateDoc,
    },
    /// An outcome the run may end in.
    Outcome {
        /// Its id.
        id: EffectId,
        /// The outcome as written.
        doc: OutcomeDoc,
    },
    /// A `may` line: an entry the Proxy lets through under a rule.
    Allow {
        /// Its id.
        id: EffectId,
        /// The rule, as `proxy.allow` keys it.
        rule: String,
        /// The entry, raw.
        entry: String,
    },
}

impl Effect {
    /// Its id.
    #[must_use]
    pub const fn id(&self) -> EffectId {
        match self {
            Self::Gate { id, .. } | Self::Outcome { id, .. } | Self::Allow { id, .. } => *id,
        }
    }

    /// The key that names the same effect in another read.
    #[must_use]
    pub fn key(&self) -> EffectKey {
        match self {
            Self::Gate { doc, .. } => EffectKey::Gate(doc.name.clone()),
            Self::Outcome { doc, .. } => EffectKey::Outcome(doc.name.to_lowercase()),
            Self::Allow { rule, entry, .. } => EffectKey::Allow {
                rule: rule.clone(),
                entry: entry.clone(),
            },
        }
    }

    /// Whether `other` holds the same thing, whatever its id.
    fn same_as(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Gate { doc: a, .. }, Self::Gate { doc: b, .. }) => a == b,
            (Self::Outcome { doc: a, .. }, Self::Outcome { doc: b, .. }) => a == b,
            _ => self.key() == other.key(),
        }
    }
}

/// An effect's kind plus one part, equal across reads for the same effect.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum EffectKey {
    /// A gate, by name.
    Gate(String),
    /// An outcome, by name in lower case.
    Outcome(String),
    /// An allow entry, by rule and entry.
    Allow {
        /// The rule.
        rule: String,
        /// The entry.
        entry: String,
    },
}

impl fmt::Display for EffectKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Gate(name) => write!(f, "gate:{name}"),
            Self::Outcome(name) => write!(f, "outcome:{name}"),
            Self::Allow { rule, entry } => write!(f, "allow:{rule}:{entry}"),
        }
    }
}

/// How one effect stands against the held read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// The held read has it as it is.
    Same,
    /// The held read has its key with other content.
    Changed,
    /// The held read lacks its key.
    New,
}

/// How a read differs from the held one, by key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comparison {
    /// Each effect of the new read, in order.
    pub each: Vec<(EffectId, Change)>,
    /// The held read's effects the new one lacks, in the held order.
    pub gone: Vec<Effect>,
}

impl Read {
    /// How this read differs from `held`, the read the automation holds now.
    #[must_use]
    pub fn compare(&self, held: &Self) -> Comparison {
        let by_key: BTreeMap<_, _> = held.effects().iter().map(|e| (e.key(), e)).collect();
        let each = self
            .effects()
            .iter()
            .map(|e| {
                let change = match by_key.get(&e.key()) {
                    None => Change::New,
                    Some(old) if old.same_as(e) => Change::Same,
                    Some(_) => Change::Changed,
                };
                (e.id(), change)
            })
            .collect();
        let mine: Vec<_> = self.effects().iter().map(Effect::key).collect();
        let gone = held
            .effects()
            .iter()
            .filter(|e| !mine.contains(&e.key()))
            .cloned()
            .collect();
        Comparison { each, gone }
    }
}
