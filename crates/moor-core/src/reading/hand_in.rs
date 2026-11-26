//! The document a reader hands in, and the check that turns it into a [`Read`]: moor places the
//! quotes, expands the intents by the table and derives each mark.

use std::collections::{BTreeMap, HashMap, HashSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::expand::{expand, Pinned};
use super::place::{place, Miss};
use super::{
    Effect, EffectId, EffectIds, Found, IntentTable, IntentUse, Line, Mark, Origin, Phrase,
    PhraseId, Polarity, Quote, Read, ReaderRun, Refusal, SendAs, Span,
};
use crate::gate::{GateDoc, Gates, OutcomeDoc};
use crate::name::ForgeRepo;

/// What a reader hands in: phrases by quote, and the gates and outcomes they hold.
///
/// Each phrase names the intents of its words. moor places the quotes, expands the intents and
/// issues the ids.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HandIn {
    /// The phrases, each quoting the prompt's own words. Phrases never share words.
    pub phrases: Vec<PhraseDoc>,
    /// The gates and outcomes moor holds the agent to. Every one must be held by a phrase.
    pub effects: Vec<EffectDoc>,
}

/// One phrase as the reader sends it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PhraseDoc {
    /// The words, with a prefix or suffix when they appear more than once.
    pub quote: Quote,
    /// The labels of the gates and outcomes it holds.
    #[serde(default)]
    pub holds: Vec<String>,
    /// The requests its words ask for or forbid.
    #[serde(default)]
    pub intents: Vec<IntentDoc>,
    /// Why, in one plain sentence shown on hover.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

/// One request a clause asks for or forbids, by intent name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IntentDoc {
    /// A name from the instructions' list, or your own when none fits.
    pub intent: String,
    /// `do` when the words ask for the request, `never` when they forbid it.
    pub polarity: Polarity,
    /// `OWNER/NAME`, only when the words name one of the Box's repos.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// `app` only when the words ask for the write to show as the GitHub App.
    #[serde(rename = "as", default)]
    pub send_as: SendAs,
}

/// One gate or outcome as the reader sends it, under a label phrases name.
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
}

impl EffectDoc {
    /// The reader's label.
    fn label(&self) -> &str {
        match self {
            Self::Gate { label, .. } | Self::Outcome { label, .. } => label,
        }
    }
}

/// What a reading is checked against besides its source.
pub(super) struct Against<'a> {
    /// The reading's repos.
    pub(super) repos: &'a [ForgeRepo],
    /// The intent table.
    pub(super) table: &'a IntentTable,
    /// The pinned rules.
    pub(super) pinned: &'a Pinned<'a>,
}

/// A phrase placed, its holds resolved and its intents expanded, before allow ids exist.
struct Pending<'a> {
    /// Where it sits.
    at: Span,
    /// As handed in.
    doc: &'a PhraseDoc,
    /// The gates and outcomes it holds.
    holds: Vec<EffectId>,
    /// Its intents, expanded.
    intents: Vec<IntentUse>,
    /// The lines its intents add.
    lines: Vec<Line>,
}

/// The read `doc` hands in for `source`, or the first thing wrong with it.
pub(super) fn check(
    doc: &HandIn,
    source: &str,
    against: &Against<'_>,
    reader: ReaderRun,
) -> Result<Read, Refusal> {
    let mut effects = effects(&doc.effects)?;
    let labels: HashMap<&str, EffectId> = doc
        .effects
        .iter()
        .zip(&effects)
        .map(|(d, e)| (d.label(), e.id()))
        .collect();
    let mut placed = doc
        .phrases
        .iter()
        .map(|p| pending(source, p, &labels, against))
        .collect::<Result<Vec<_>, _>>()?;
    placed.sort_by_key(|p| p.at.start());
    if let Some([a, b]) = placed
        .windows(2)
        .find(|w| matches!(w, [x, y] if x.at.end() > y.at.start()))
    {
        return Err(Refusal::Overlap {
            first: a.doc.quote.exact.clone(),
            second: b.doc.quote.exact.clone(),
        });
    }
    let mut allows: BTreeMap<Line, EffectId> = BTreeMap::new();
    let phrases: Vec<Phrase> = placed
        .into_iter()
        .enumerate()
        .map(|(at, p)| settle(PhraseId::nth(at), p, &mut effects, &mut allows))
        .collect();
    let held: HashSet<EffectId> = phrases
        .iter()
        .filter_map(|p| match &p.mark {
            Mark::Held { effects } => Some(effects.iter()),
            Mark::Asked {} | Mark::Unmapped {} => None,
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

/// The phrase `p` with its mark: held when every intent found its lines and it holds at least one
/// effect, its lines joining `effects` once each by `allows`; else unmapped or asked, holding nothing.
fn settle(
    id: PhraseId,
    p: Pending<'_>,
    effects: &mut Vec<Effect>,
    allows: &mut BTreeMap<Line, EffectId>,
) -> Phrase {
    let whole = p
        .intents
        .iter()
        .all(|i| matches!(i.found, Found::Lines { .. }));
    let mark = if whole {
        let mut ids = p.holds;
        for line in p.lines {
            let next = EffectId::nth(effects.len());
            let id = *allows.entry(line.clone()).or_insert_with(|| {
                effects.push(Effect::Allow {
                    id: next,
                    rule: line.rule,
                    entry: line.entry,
                    from: Origin::Table,
                });
                next
            });
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        EffectIds::try_from(ids).map_or(Mark::Asked {}, |effects| Mark::Held { effects })
    } else if p.intents.iter().any(|i| i.found == Found::NoRow) {
        Mark::Unmapped {}
    } else {
        Mark::Asked {}
    };
    Phrase {
        id,
        quote: p.doc.quote.clone(),
        at: p.at,
        mark,
        intents: p.intents,
        why: p.doc.why.clone(),
    }
}

/// The phrase `p`, placed in `source`, its holds resolved by `labels` and its intents expanded.
fn pending<'a>(
    source: &str,
    p: &'a PhraseDoc,
    labels: &HashMap<&str, EffectId>,
    against: &Against<'_>,
) -> Result<Pending<'a>, Refusal> {
    let quote = || p.quote.exact.clone();
    let at = place(source, &p.quote).map_err(|miss| match miss {
        Miss::Empty => Refusal::EmptyQuote { quote: quote() },
        Miss::NotFound => Refusal::NotFound { quote: quote() },
        Miss::Ambiguous(found) => Refusal::Ambiguous {
            quote: quote(),
            found,
        },
    })?;
    let mut holds = Vec::with_capacity(p.holds.len());
    for label in &p.holds {
        let id = *labels
            .get(label.as_str())
            .ok_or_else(|| Refusal::UnknownEffect {
                quote: quote(),
                label: label.clone(),
            })?;
        if holds.contains(&id) {
            return Err(Refusal::RepeatedEffect {
                quote: quote(),
                label: label.clone(),
            });
        }
        holds.push(id);
    }
    let mut intents = Vec::with_capacity(p.intents.len());
    let mut lines = Vec::new();
    for doc in &p.intents {
        let (used, added) = expand(
            doc,
            &p.quote.exact,
            against.table,
            against.pinned,
            against.repos,
        )?;
        intents.push(used);
        lines.extend(added);
    }
    Ok(Pending {
        at,
        doc: p,
        holds,
        intents,
        lines,
    })
}

/// The gates and outcomes `docs` describe, with moor's ids, each checked.
fn effects(docs: &[EffectDoc]) -> Result<Vec<Effect>, Refusal> {
    let mut labels = HashSet::new();
    let mut keys = HashSet::new();
    let mut out = Vec::with_capacity(docs.len());
    for (at, d) in docs.iter().enumerate() {
        if !labels.insert(d.label()) {
            return Err(Refusal::DuplicateLabel {
                label: d.label().to_owned(),
            });
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
        };
        if !keys.insert(effect.key()) {
            return Err(Refusal::DuplicateKey {
                label: d.label().to_owned(),
            });
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
            EffectDoc::Outcome { .. } => None,
        })
        .collect();
    let outcomes: Vec<(&str, OutcomeDoc)> = docs
        .iter()
        .filter_map(|d| match d {
            EffectDoc::Outcome { label, outcome } => Some((label.as_str(), outcome.clone())),
            EffectDoc::Gate { .. } => None,
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
