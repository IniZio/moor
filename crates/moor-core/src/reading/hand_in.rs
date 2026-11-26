//! The document a reader hands in, and the check that turns it into a [`Read`].

use std::collections::{HashMap, HashSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::place::{place, Miss};
use super::{
    Effect, EffectId, EffectIds, Mark, Phrase, PhraseId, ProxyAllow, Quote, Read, ReaderRun,
    Refusal, Span,
};
use crate::gate::{GateDoc, Gates, OutcomeDoc};

/// What a reader hands in: phrases by quote, and the effects they hold under labels of its own.
/// moor places the quotes and issues the ids.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HandIn {
    /// The phrases, each quoting the prompt's own words. Phrases never share words.
    pub phrases: Vec<PhraseDoc>,
    /// What moor holds the agent to. Every effect must be named by a held phrase.
    pub effects: Vec<EffectDoc>,
}

/// One phrase as the reader sends it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PhraseDoc {
    /// The words, with a prefix or suffix when they appear more than once.
    pub quote: Quote,
    /// What moor holds of them.
    pub mark: MarkDoc,
    /// Why, in one plain sentence shown on hover.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

/// What moor holds of a phrase, as the reader sends it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MarkDoc {
    /// moor holds the agent to these effects, by label: at least one.
    Held {
        /// The labels of the effects.
        effects: Vec<String>,
    },
    /// Only asked of the agent.
    Asked {},
}

/// One effect as the reader sends it, under a label phrases name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EffectDoc {
    /// A gate the run's Box must pass.
    Gate {
        /// Your label for it.
        label: String,
        /// The gate.
        gate: GateDoc,
    },
    /// An outcome the run may end in.
    Outcome {
        /// Your label for it.
        label: String,
        /// The outcome.
        outcome: OutcomeDoc,
    },
    /// An entry the Proxy lets through under a rule, exactly as the workflow's `proxy.allow` lists it.
    Allow {
        /// Your label for it.
        label: String,
        /// The rule name.
        rule: String,
        /// The entry.
        entry: String,
    },
}

impl EffectDoc {
    /// The reader's label.
    fn label(&self) -> &str {
        match self {
            Self::Gate { label, .. } | Self::Outcome { label, .. } | Self::Allow { label, .. } => {
                label
            }
        }
    }
}

/// The read `doc` hands in for `source`, or the first thing wrong with it.
pub(super) fn check(
    doc: &HandIn,
    source: &str,
    allow: &ProxyAllow,
    reader: ReaderRun,
) -> Result<Read, Refusal> {
    let effects = effects(&doc.effects, allow)?;
    let labels: HashMap<&str, EffectId> = doc
        .effects
        .iter()
        .zip(&effects)
        .map(|(d, e)| (d.label(), e.id()))
        .collect();
    let mut placed = doc
        .phrases
        .iter()
        .map(|p| placed(source, p))
        .collect::<Result<Vec<_>, _>>()?;
    placed.sort_by_key(|(at, _)| at.start());
    if let Some([(_, a), (_, b)]) = placed
        .windows(2)
        .find(|w| matches!(w, [(x, _), (y, _)] if x.end() > y.start()))
    {
        return Err(Refusal::Overlap {
            first: a.quote.exact.clone(),
            second: b.quote.exact.clone(),
        });
    }
    let phrases = placed
        .into_iter()
        .enumerate()
        .map(|(at, (span, p))| {
            Ok(Phrase {
                id: PhraseId::nth(at),
                quote: p.quote.clone(),
                at: span,
                mark: mark(p, &labels)?,
                why: p.why.clone(),
            })
        })
        .collect::<Result<Vec<_>, Refusal>>()?;
    let held: HashSet<EffectId> = phrases
        .iter()
        .filter_map(|p| match &p.mark {
            Mark::Held { effects } => Some(effects.iter()),
            Mark::Asked {} => None,
        })
        .flatten()
        .copied()
        .collect();
    if let Some((d, _)) = doc
        .effects
        .iter()
        .zip(&effects)
        .find(|(_, e)| !held.contains(&e.id()))
    {
        return Err(Refusal::Unheld {
            label: d.label().to_owned(),
        });
    }
    Ok(Read::new(phrases, effects, Some(reader)))
}

/// The phrase `p` and where its quote sits in `source`.
fn placed<'a>(source: &str, p: &'a PhraseDoc) -> Result<(Span, &'a PhraseDoc), Refusal> {
    let quote = || p.quote.exact.clone();
    let at = place(source, &p.quote).map_err(|miss| match miss {
        Miss::Empty => Refusal::EmptyQuote { quote: quote() },
        Miss::NotFound => Refusal::NotFound { quote: quote() },
        Miss::Ambiguous(found) => Refusal::Ambiguous {
            quote: quote(),
            found,
        },
    })?;
    Ok((at, p))
}

/// The mark of `p`, its labels turned into moor's effect ids.
fn mark(p: &PhraseDoc, labels: &HashMap<&str, EffectId>) -> Result<Mark, Refusal> {
    let MarkDoc::Held { effects } = &p.mark else {
        return Ok(Mark::Asked {});
    };
    let quote = || p.quote.exact.clone();
    let mut ids = Vec::with_capacity(effects.len());
    for label in effects {
        let id = *labels
            .get(label.as_str())
            .ok_or_else(|| Refusal::UnknownEffect {
                quote: quote(),
                label: label.clone(),
            })?;
        if ids.contains(&id) {
            return Err(Refusal::RepeatedEffect {
                quote: quote(),
                label: label.clone(),
            });
        }
        ids.push(id);
    }
    EffectIds::try_from(ids)
        .map(|effects| Mark::Held { effects })
        .map_err(|_| Refusal::HeldWithoutEffect { quote: quote() })
}

/// The effects `docs` describe, with moor's ids, each checked.
fn effects(docs: &[EffectDoc], allow: &ProxyAllow) -> Result<Vec<Effect>, Refusal> {
    let mut labels = HashSet::new();
    let mut keys = HashSet::new();
    let mut out = Vec::with_capacity(docs.len());
    for (at, d) in docs.iter().enumerate() {
        let label = || d.label().to_owned();
        if !labels.insert(d.label()) {
            return Err(Refusal::DuplicateLabel { label: label() });
        }
        let id = EffectId::nth(at);
        let effect = match d {
            EffectDoc::Gate { gate, .. } => Effect::Gate {
                id,
                doc: gate.clone(),
            },
            EffectDoc::Outcome { outcome, .. } => Effect::Outcome {
                id,
                doc: outcome.clone(),
            },
            EffectDoc::Allow { rule, entry, .. } => {
                if !allow.get(rule).is_some_and(|listed| listed.contains(entry)) {
                    return Err(Refusal::NotAllowed {
                        label: label(),
                        rule: rule.clone(),
                        entry: entry.clone(),
                    });
                }
                Effect::Allow {
                    id,
                    rule: rule.clone(),
                    entry: entry.clone(),
                }
            }
        };
        if !keys.insert(effect.key()) {
            return Err(Refusal::DuplicateKey { label: label() });
        }
        out.push(effect);
    }
    check_gates(docs)?;
    Ok(out)
}

/// Refuses gates and outcomes that do not parse, and outcomes with no gate.
fn check_gates(docs: &[EffectDoc]) -> Result<(), Refusal> {
    let gates: Vec<GateDoc> = docs
        .iter()
        .filter_map(|d| match d {
            EffectDoc::Gate { gate, .. } => Some(gate.clone()),
            _ => None,
        })
        .collect();
    let outcomes: Vec<(&str, OutcomeDoc)> = docs
        .iter()
        .filter_map(|d| match d {
            EffectDoc::Outcome { label, outcome } => Some((label.as_str(), outcome.clone())),
            _ => None,
        })
        .collect();
    match (gates.is_empty(), outcomes.first()) {
        (true, Some((label, _))) => Err(Refusal::OutcomeWithoutGate {
            label: (*label).to_owned(),
        }),
        (true, None) => Ok(()),
        (false, _) => {
            let outcomes: Vec<_> = outcomes.into_iter().map(|(_, o)| o).collect();
            Gates::parse(&gates, &outcomes)
                .map(drop)
                .map_err(Refusal::Gates)
        }
    }
}
