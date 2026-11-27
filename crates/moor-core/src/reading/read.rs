//! A finished read: phrases placed in the source and the effects they hold.

use std::collections::HashSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{Effect, EffectId, IntentUse, PhraseId, ReaderRun};
use crate::gate::GatesDoc;

#[cfg(test)]
mod tests;

/// A read: phrases in source order that never overlap, and the effects they hold.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Stored")]
pub struct Read {
    /// In source order, never overlapping.
    phrases: Vec<Phrase>,
    /// In order: gates pass in this order.
    effects: Vec<Effect>,
    /// Who read it; none for a read built from stored gates.
    #[serde(skip_serializing_if = "Option::is_none")]
    reader: Option<ReaderRun>,
}

/// One cut of a source: its text, and the phrase it is when it is one.
pub type Run<'a> = (&'a str, Option<PhraseId>);

impl Read {
    /// A read of parts already checked.
    pub(super) const fn new(
        phrases: Vec<Phrase>,
        effects: Vec<Effect>,
        reader: Option<ReaderRun>,
    ) -> Self {
        Self {
            phrases,
            effects,
            reader,
        }
    }

    /// The gates and outcomes of `doc` as effects, with no phrases: how stored gates read.
    #[must_use]
    pub fn from_gates(doc: &GatesDoc) -> Self {
        let mut effects = Vec::with_capacity(doc.gates.len() + doc.outcomes.len());
        for doc in &doc.gates {
            let id = EffectId::nth(effects.len());
            effects.push(Effect::Gate {
                id,
                doc: doc.clone(),
            });
        }
        for doc in &doc.outcomes {
            let id = EffectId::nth(effects.len());
            effects.push(Effect::Outcome {
                id,
                doc: doc.clone(),
            });
        }
        Self::new(Vec::new(), effects, None)
    }

    /// The phrases, in source order.
    #[must_use]
    pub fn phrases(&self) -> &[Phrase] {
        &self.phrases
    }

    /// The effects, in order.
    #[must_use]
    pub fn effects(&self) -> &[Effect] {
        &self.effects
    }

    /// Who read it; none for a read built from stored gates.
    #[must_use]
    pub const fn reader(&self) -> Option<&ReaderRun> {
        self.reader.as_ref()
    }

    /// The gates and outcomes, in effect order; none when it holds no gate.
    #[must_use]
    pub fn gates(&self) -> Option<GatesDoc> {
        let gates: Vec<_> = self
            .effects
            .iter()
            .filter_map(|e| match e {
                Effect::Gate { doc, .. } => Some(doc.clone()),
                _ => None,
            })
            .collect();
        let outcomes = self
            .effects
            .iter()
            .filter_map(|e| match e {
                Effect::Outcome { doc, .. } => Some(doc.clone()),
                _ => None,
            })
            .collect();
        (!gates.is_empty()).then_some(GatesDoc {
            gates,
            outcomes,
            made_from: None,
        })
    }

    /// `source` cut at phrase edges, so a front draws marks with no offset math; none when the
    /// phrases do not fit `source`, which is then not the text read.
    #[must_use]
    pub fn runs<'a>(&self, source: &'a str) -> Option<Vec<Run<'a>>> {
        let mut out = Vec::with_capacity(self.phrases.len() * 2 + 1);
        let mut at = 0;
        for p in &self.phrases {
            let gap = source.get(at..p.at.start)?;
            if !gap.is_empty() {
                out.push((gap, None));
            }
            out.push((source.get(p.at.start..p.at.end)?, Some(p.id)));
            at = p.at.end;
        }
        let tail = source.get(at..)?;
        if !tail.is_empty() {
            out.push((tail, None));
        }
        Some(out)
    }
}

/// One phrase of the source and what moor made of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Phrase {
    /// Its id.
    pub id: PhraseId,
    /// What the reader sent.
    pub quote: Quote,
    /// Where moor placed it.
    pub at: Span,
    /// What moor holds of it.
    pub mark: Mark,
    /// The requests its words ask for or forbid, with what expansion found.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub intents: Vec<IntentUse>,
    /// The reader's own words, shown on hover.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

/// Words of the source as the reader quoted them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Quote {
    /// The words, copied from the prompt.
    pub exact: String,
    /// The words just before, when the same words appear more than once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    /// The words just after, when the same words appear more than once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suffix: Option<String>,
}

/// UTF-8 byte offsets into the source, each on a char boundary, start before end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    /// The first byte.
    start: usize,
    /// One past the last byte.
    end: usize,
}

impl Span {
    /// The span from `start` to `end`.
    pub(super) const fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    /// The first byte.
    #[must_use]
    pub const fn start(&self) -> usize {
        self.start
    }

    /// One past the last byte.
    #[must_use]
    pub const fn end(&self) -> usize {
        self.end
    }
}

/// What moor holds of a phrase; the underline style follows from the variant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Mark {
    /// Solid: moor holds the agent to every effect.
    Held {
        /// The effects, at least one.
        effects: EffectIds,
    },
    /// Dashed: only asked of the agent.
    Asked {},
    /// "moor has no rule for this": a request no row covers. Holds nothing.
    Unmapped {},
}

/// The effects a held phrase holds: at least one, none twice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Vec<EffectId>", into = "Vec<EffectId>")]
pub struct EffectIds(Vec<EffectId>);

impl EffectIds {
    /// The effects, in the order the reader listed them.
    pub fn iter(&self) -> std::slice::Iter<'_, EffectId> {
        self.0.iter()
    }
}

impl TryFrom<Vec<EffectId>> for EffectIds {
    type Error = &'static str;

    fn try_from(ids: Vec<EffectId>) -> Result<Self, Self::Error> {
        let distinct = ids.iter().collect::<HashSet<_>>().len() == ids.len();
        if ids.is_empty() || !distinct {
            return Err("a held phrase needs at least one effect, none twice");
        }
        Ok(Self(ids))
    }
}

impl<'a> IntoIterator for &'a EffectIds {
    type Item = &'a EffectId;
    type IntoIter = std::slice::Iter<'a, EffectId>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl From<EffectIds> for Vec<EffectId> {
    fn from(ids: EffectIds) -> Self {
        ids.0
    }
}

/// A read as JSON keeps it, before its rules are checked.
#[derive(Deserialize)]
struct Stored {
    /// See [`Read`].
    phrases: Vec<Phrase>,
    /// See [`Read`].
    effects: Vec<Effect>,
    /// See [`Read`].
    #[serde(default)]
    reader: Option<ReaderRun>,
}

impl TryFrom<Stored> for Read {
    type Error = &'static str;

    fn try_from(s: Stored) -> Result<Self, Self::Error> {
        let ordered = s.phrases.iter().all(|p| p.at.start < p.at.end)
            && s.phrases
                .windows(2)
                .all(|w| matches!(w, [a, b] if a.at.end <= b.at.start));
        if !ordered {
            return Err("phrases out of order or overlapping");
        }
        let ids: HashSet<_> = s.effects.iter().map(Effect::id).collect();
        let phrase_ids: HashSet<_> = s.phrases.iter().map(|p| p.id).collect();
        if ids.len() != s.effects.len() || phrase_ids.len() != s.phrases.len() {
            return Err("an id used twice");
        }
        let held: HashSet<_> = s
            .phrases
            .iter()
            .filter_map(|p| match &p.mark {
                Mark::Held { effects } => Some(effects.iter()),
                Mark::Asked {} | Mark::Unmapped {} => None,
            })
            .flatten()
            .copied()
            .collect();
        if !held.is_subset(&ids) {
            return Err("a phrase holds an effect the read lacks");
        }
        let whole = if s.reader.is_some() {
            held == ids
        } else {
            s.phrases.is_empty()
        };
        if !whole {
            return Err("an effect no phrase holds");
        }
        Ok(Self::new(s.phrases, s.effects, s.reader))
    }
}
