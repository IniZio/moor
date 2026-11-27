//! `ReadingView`: what moor made of an automation's prompt, as the portal draws it.
//!
//! `docs/specs/prompt-reading.md`, "The API", is the shape: held, asked and unmapped phrases with
//! their intents, gate, outcome and allow effects with where each allow came from, and the
//! config's hand-written lines tagged against the reading.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::config::proxy::ToolRule;
use crate::gate::GatesDoc;
use crate::name::ForgeRepo;
use crate::reading::{
    Context, Effect, EffectId, IntentTable, Mark, ProxyAllow, Read, Reading, ReadingState, RulesPin,
};
use crate::workflow::PromptHash;

mod effects;
mod intents;
mod kinds;
#[cfg(test)]
mod tests;

pub use effects::EffectView;
use effects::{effect, listed, may};
pub use intents::{HandwrittenView, IntentView, Tag};
pub use kinds::{EffectKind, FromView, MarkView, Stale, ViewState};

/// Who read it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReaderView {
    /// The agent harness.
    pub harness: String,
    /// The model, when the harness named one.
    pub model: Option<String>,
    /// The version of the reader instructions it was given.
    pub instructions: u32,
}

/// One cut of the source: its text, and the phrase it is when it is one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunView {
    /// The text; the texts joined are the source.
    pub text: String,
    /// The phrase this cut is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phrase: Option<String>,
}

/// One phrase and what moor made of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhraseView {
    /// Its id, such as `p1`.
    pub id: String,
    /// Solid or dashed.
    pub mark: MarkView,
    /// The effects it lights.
    pub effects: Vec<String>,
    /// The requests its words ask for or forbid.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub intents: Vec<IntentView>,
    /// The reader's own words, shown on hover.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

/// One reading of one prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadingView {
    /// The reading's id; none for a view built from stored gates.
    pub id: Option<String>,
    /// Where it stands.
    pub state: ViewState,
    /// Why no read came.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed: Option<String>,
    /// What differs now from what the reader saw; empty when current.
    #[serde(default)]
    pub stale: Vec<Stale>,
    /// The intent table version it was expanded with; none before intents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table: Option<u32>,
    /// The words read.
    pub source: String,
    /// Who read it; none for a view built from stored gates.
    pub reader: Option<ReaderView>,
    /// The source cut at phrase edges, so a front draws marks with no offsets.
    pub runs: Vec<RunView>,
    /// The phrases, in source order.
    pub phrases: Vec<PhraseView>,
    /// The effects, in order.
    pub effects: Vec<EffectView>,
    /// The gates the gate engine gets.
    pub gates: Option<GatesDoc>,
    /// The extra Proxy entries by rule.
    pub may: BTreeMap<String, Vec<String>>,
    /// The config's `proxy.allow` lines, tagged against an intent reading's lines.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub handwritten: Vec<HandwrittenView>,
}

/// What is true now, to tell what a reading was read against from what is.
#[derive(Debug, Clone, Copy)]
pub struct Current<'a> {
    /// The automation's prompt.
    pub prompt: &'a str,
    /// Its repos, in order.
    pub repos: &'a [ForgeRepo],
    /// The rules texts a Box of it follows now.
    pub rules: &'a [RulesPin],
    /// The version of moor's reader instructions.
    pub instructions: u32,
    /// The workflow config's `proxy.allow`.
    pub allow: &'a ProxyAllow,
    /// The rules text a Box of it follows now, by rule id.
    pub rules_text: &'a BTreeMap<String, ToolRule>,
    /// The intent table moor runs; none when it did not load.
    pub table: Option<&'a IntentTable>,
}

impl ReadingView {
    /// `reading` as shown against what is true `now`. An allow effect whose entry `now.allow`
    /// no longer lists is dropped, and its phrase shows asked when it held nothing else.
    #[must_use]
    pub fn of(reading: &Reading, now: &Current<'_>) -> Self {
        let (state, failed, reader, shown) = match &reading.state {
            ReadingState::Read(read) => (
                ViewState::Read,
                None,
                read.reader().cloned(),
                shown(read, &reading.source, now.allow, now.table),
            ),
            ReadingState::Reading { by, .. } => (
                ViewState::Reading,
                None,
                Some(by.clone()),
                Shown::none(&reading.source),
            ),
            ReadingState::Failed { why, .. } => (
                ViewState::Failed,
                Some(why.clone()),
                None,
                Shown::none(&reading.source),
            ),
        };
        Self {
            id: Some(reading.id.to_string()),
            state,
            failed,
            stale: stale(&reading.context, now, &reading.state),
            table: reading.context.table.map(|t| t.0),
            source: reading.source.clone(),
            reader: reader.map(|r| ReaderView {
                harness: r.harness,
                model: r.model,
                instructions: reading.context.instructions,
            }),
            runs: shown.runs,
            phrases: shown.phrases,
            effects: shown.effects,
            gates: shown.gates,
            may: shown.may,
            handwritten: match (&reading.state, reading.context.table) {
                (ReadingState::Read(read), Some(_)) => {
                    intents::handwritten(read, now.allow, now.rules_text)
                }
                _ => Vec::new(),
            },
        }
    }

    /// The gates `doc` a workflow stores with no reading, read as effects and no phrases over
    /// its `prompt`.
    #[must_use]
    pub fn from_gates(prompt: &str, doc: &GatesDoc) -> Self {
        let shown = shown(&Read::from_gates(doc), prompt, &ProxyAllow::new(), None);
        Self {
            id: None,
            state: ViewState::Read,
            failed: None,
            stale: Vec::new(),
            table: None,
            source: prompt.to_owned(),
            reader: None,
            runs: shown.runs,
            phrases: shown.phrases,
            effects: shown.effects,
            gates: shown.gates,
            may: shown.may,
            handwritten: Vec::new(),
        }
    }
}

/// The parts of a view that come from a read.
struct Shown {
    /// The runs.
    runs: Vec<RunView>,
    /// The phrases.
    phrases: Vec<PhraseView>,
    /// The effects.
    effects: Vec<EffectView>,
    /// The gates.
    gates: Option<GatesDoc>,
    /// The allow entries by rule.
    may: BTreeMap<String, Vec<String>>,
}

impl Shown {
    /// A source with nothing read from it.
    fn none(source: &str) -> Self {
        Self {
            runs: whole(source),
            phrases: Vec::new(),
            effects: Vec::new(),
            gates: None,
            may: BTreeMap::new(),
        }
    }
}

/// `source` as one unmarked run; none for no text.
fn whole(source: &str) -> Vec<RunView> {
    if source.is_empty() {
        return Vec::new();
    }
    vec![RunView {
        text: source.to_owned(),
        phrase: None,
    }]
}

/// `read` of `source`, less the slice-1 allow effects `allow` no longer lists. Phrases that do
/// not fit `source` are not the words read, so they draw no marks.
fn shown(read: &Read, source: &str, allow: &ProxyAllow, table: Option<&IntentTable>) -> Shown {
    let kept: Vec<&Effect> = read.effects().iter().filter(|e| listed(e, allow)).collect();
    let base = Shown {
        gates: read.gates(),
        may: may(&kept),
        ..Shown::none(source)
    };
    let Some(cut) = read.runs(source) else {
        return Shown {
            effects: kept.iter().map(|e| effect(e, &[])).collect(),
            ..base
        };
    };
    let alive: HashSet<EffectId> = kept.iter().map(|e| e.id()).collect();
    let phrases = phrases(read, &alive, table);
    Shown {
        runs: cut
            .into_iter()
            .map(|(text, phrase)| RunView {
                text: text.to_owned(),
                phrase: phrase.map(|p| p.to_string()),
            })
            .collect(),
        effects: kept
            .iter()
            .map(|e| {
                let id = e.id().to_string();
                let held_by: Vec<String> = phrases
                    .iter()
                    .filter(|p| p.effects.contains(&id))
                    .map(|p| p.id.clone())
                    .collect();
                effect(e, &held_by)
            })
            .collect(),
        phrases,
        ..base
    }
}

/// The phrases of `read` holding only the effects in `alive`; one left holding none is asked,
/// unless it names a request no row covers.
fn phrases(read: &Read, alive: &HashSet<EffectId>, table: Option<&IntentTable>) -> Vec<PhraseView> {
    read.phrases()
        .iter()
        .map(|p| {
            let effects: Vec<String> = match &p.mark {
                Mark::Held { effects } => effects
                    .iter()
                    .filter(|e| alive.contains(e))
                    .map(ToString::to_string)
                    .collect(),
                Mark::Asked { .. } | Mark::Unmapped {} => Vec::new(),
            };
            PhraseView {
                id: p.id.to_string(),
                mark: match (&p.mark, effects.is_empty()) {
                    (Mark::Unmapped {}, _) => MarkView::Unmapped,
                    (_, true) => MarkView::Asked,
                    (_, false) => MarkView::Held,
                },
                effects,
                intents: p
                    .intents
                    .iter()
                    .map(|u| intents::intent(u, table))
                    .collect(),
                why: p.why.clone(),
            }
        })
        .collect()
}

/// What of `ctx` differs from `now`, in the order of [`Stale`]; a newer table counts only when
/// it changes what `state` read.
fn stale(ctx: &Context, now: &Current<'_>, state: &ReadingState) -> Vec<Stale> {
    let table = match (state, ctx.table, now.table) {
        (ReadingState::Read(read), Some(was), Some(is)) => intents::table_changes(read, was, is),
        _ => false,
    };
    [
        (ctx.source_hash != PromptHash::of(now.prompt), Stale::Prompt),
        (ctx.repos != now.repos, Stale::Repos),
        (ctx.rules != now.rules, Stale::Rules),
        (ctx.instructions != now.instructions, Stale::Reader),
        (table, Stale::Table),
    ]
    .into_iter()
    .filter_map(|(differs, why)| differs.then_some(why))
    .collect()
}
