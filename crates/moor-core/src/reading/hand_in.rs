//! The document a reader hands in, and the check that turns it into a [`Read`]: moor places the
//! quotes, expands the intents by the table and derives each mark.

use std::collections::{BTreeMap, HashMap, HashSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use effects::Effects;

use super::expand::{expand, Pinned};
use super::never::let_through;
use super::place::{place, Miss};
use super::{
    Effect, EffectId, EffectIds, Found, IntentTable, IntentUse, Line, Mark, Phrase, PhraseId,
    Polarity, Quote, Read, ReaderRun, Refusal, Request, SendAs, Span,
};
use crate::gate::{GateDoc, Gates, OutcomeDoc};
use crate::name::ForgeRepo;

mod effects;

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
    /// The requests each intent forbids, by the place of the intent; empty for a `do` one.
    forbids: Vec<Vec<Request>>,
}

impl Pending<'_> {
    /// Each `never` intent checked against `pinned` and the reading's `own` lines: still
    /// refused, or let through, naming the request and the rule, and then forbidding nothing.
    fn forbid(&mut self, pinned: &Pinned<'_>, own: &[Line]) {
        for (u, requests) in self.intents.iter_mut().zip(&mut self.forbids) {
            if u.found != Found::Refused {
                continue;
            }
            let repo = u.repo.as_ref().map(ToString::to_string);
            if let Some(allowed) = let_through(requests, repo.as_deref(), pinned, own) {
                u.found = Found::LetThrough(allowed);
                requests.clear();
            }
        }
    }
}

/// The read `doc` hands in for `source`, or the first thing wrong with it.
pub(super) fn check(
    doc: &HandIn,
    source: &str,
    against: &Against<'_>,
    reader: ReaderRun,
) -> Result<Read, Refusal> {
    let given = effects(&doc.effects)?;
    let labels: HashMap<&str, EffectId> = doc
        .effects
        .iter()
        .zip(&given)
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
    let own: Vec<Line> = placed.iter().flat_map(|p| p.lines.clone()).collect();
    for p in &mut placed {
        p.forbid(against.pinned, &own);
    }
    let mut all = Effects {
        list: given,
        allows: BTreeMap::new(),
        withheld: BTreeMap::new(),
    };
    let phrases: Vec<Phrase> = placed
        .into_iter()
        .enumerate()
        .map(|(at, p)| settle(PhraseId::nth(at), p, &mut all))
        .collect();
    let effects = all.list;
    let held: HashSet<EffectId> = phrases
        .iter()
        .filter_map(|p| match &p.mark {
            Mark::Held { effects } => Some(effects.iter()),
            Mark::Asked { .. } | Mark::Unmapped {} => None,
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

/// The phrase `p` with its mark, settled per intent. An intent no row covers holds nothing and
/// stays on the phrase as "moor has no rule for this"; the phrase is then unmapped unless another
/// intent gives a line or a withheld request. When every other intent found its lines or has its
/// requests refused, the phrase is held by them and its gates, each joining `all` once, or asked
/// with nothing to hold. An intent let through or unholdable leaves the phrase asked, naming what
/// the rules let through, holding nothing.
fn settle(id: PhraseId, p: Pending<'_>, all: &mut Effects) -> Phrase {
    let holdable = p
        .intents
        .iter()
        .all(|i| matches!(i.found, Found::Lines { .. } | Found::Refused | Found::NoRow));
    let unmapped = p.intents.iter().any(|i| i.found == Found::NoRow);
    let gives = !p.lines.is_empty() || p.forbids.iter().any(|r| !r.is_empty());
    let mark = if !holdable {
        let allowed = p.intents.iter().find_map(|i| match &i.found {
            Found::LetThrough(a) => Some(a.clone()),
            _ => None,
        });
        Mark::Asked { allowed }
    } else if unmapped && !gives {
        Mark::Unmapped {}
    } else {
        let mut ids = p.holds;
        let mut added: Vec<EffectId> = p.lines.into_iter().map(|l| all.allow(l)).collect();
        added.extend(p.forbids.into_iter().flatten().map(|r| all.withhold(r)));
        for id in added {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        EffectIds::try_from(ids).map_or(Mark::Asked { allowed: None }, |effects| Mark::Held {
            effects,
        })
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
    let mut forbids = Vec::with_capacity(p.intents.len());
    for doc in &p.intents {
        let e = expand(
            doc,
            &p.quote.exact,
            against.table,
            against.pinned,
            against.repos,
        )?;
        intents.push(e.used);
        lines.extend(e.adds);
        forbids.push(e.forbids);
    }
    Ok(Pending {
        at,
        doc: p,
        holds,
        intents,
        lines,
        forbids,
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
