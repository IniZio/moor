//! `ReadingView`: what moor made of an automation's prompt, as the portal draws it.
//!
//! `docs/specs/prompt-reading.md`, "The API", is the shape; slice 1 sends held and asked phrases and
//! gate, outcome and allow effects.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::gate::GatesDoc;
use crate::name::ForgeRepo;
use crate::reading::{
    Context, Effect, EffectId, Mark, ProxyAllow, Read, Reading, ReadingState, RulesPin,
};
use crate::workflow::PromptHash;

#[cfg(test)]
mod tests;

/// Where the reading stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewState {
    /// A reader is reading.
    Reading,
    /// Read.
    Read,
    /// No read came.
    Failed,
}

/// What differs now from what the reader saw.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stale {
    /// The prompt changed.
    Prompt,
    /// The repo list changed.
    Repos,
    /// A rules text the reading pinned was saved again.
    Rules,
    /// moor's reader instructions changed.
    Reader,
}

/// The underline a phrase draws: solid or dashed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarkView {
    /// Solid: moor holds the agent to it.
    Held,
    /// Dashed: only asked.
    Asked,
}

/// What an effect is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectKind {
    /// A gate.
    Gate,
    /// An outcome.
    Outcome,
    /// A `may` line.
    Allow,
}

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
    /// The reader's own words, shown on hover.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

/// One thing moor holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectView {
    /// Its id, such as `e1`.
    pub id: String,
    /// What it is.
    pub kind: EffectKind,
    /// What the file or the gate list shows: an allow's entry, a gate's or outcome's name.
    pub line: String,
    /// Equal across readings for the same effect.
    pub key: String,
    /// The phrases that hold it.
    pub phrases: Vec<String>,
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
                shown(read, &reading.source, now.allow),
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
            stale: stale(&reading.context, now),
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
        }
    }

    /// The gates `doc` a workflow stores with no reading, read as effects and no phrases over
    /// its `prompt`.
    #[must_use]
    pub fn from_gates(prompt: &str, doc: &GatesDoc) -> Self {
        let shown = shown(&Read::from_gates(doc), prompt, &ProxyAllow::new());
        Self {
            id: None,
            state: ViewState::Read,
            failed: None,
            stale: Vec::new(),
            source: prompt.to_owned(),
            reader: None,
            runs: shown.runs,
            phrases: shown.phrases,
            effects: shown.effects,
            gates: shown.gates,
            may: shown.may,
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

/// `read` of `source`, less the allow effects `allow` no longer lists. Phrases that do not fit
/// `source` are not the words read, so they draw no marks.
fn shown(read: &Read, source: &str, allow: &ProxyAllow) -> Shown {
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
    let phrases = phrases(read, &alive);
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

/// Whether the Proxy still gets `e`: an allow effect needs its entry in `allow`.
fn listed(e: &Effect, allow: &ProxyAllow) -> bool {
    match e {
        Effect::Allow { rule, entry, .. } => allow
            .get(rule)
            .is_some_and(|entries| entries.contains(entry)),
        Effect::Gate { .. } | Effect::Outcome { .. } => true,
    }
}

/// The phrases of `read` holding only the effects in `alive`; one left holding none is asked.
fn phrases(read: &Read, alive: &HashSet<EffectId>) -> Vec<PhraseView> {
    read.phrases()
        .iter()
        .map(|p| {
            let effects: Vec<String> = match &p.mark {
                Mark::Held { effects } => effects
                    .iter()
                    .filter(|e| alive.contains(e))
                    .map(ToString::to_string)
                    .collect(),
                Mark::Asked {} => Vec::new(),
            };
            PhraseView {
                id: p.id.to_string(),
                mark: if effects.is_empty() {
                    MarkView::Asked
                } else {
                    MarkView::Held
                },
                effects,
                why: p.why.clone(),
            }
        })
        .collect()
}

/// `e` as shown, held by `phrases`.
fn effect(e: &Effect, phrases: &[String]) -> EffectView {
    let (kind, line) = match e {
        Effect::Gate { doc, .. } => (EffectKind::Gate, doc.name.clone()),
        Effect::Outcome { doc, .. } => (EffectKind::Outcome, doc.name.clone()),
        Effect::Allow { entry, .. } => (EffectKind::Allow, entry.clone()),
    };
    EffectView {
        id: e.id().to_string(),
        kind,
        line,
        key: e.key().to_string(),
        phrases: phrases.to_vec(),
    }
}

/// The allow entries of `effects` by rule, in effect order.
fn may(effects: &[&Effect]) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for e in effects {
        if let Effect::Allow { rule, entry, .. } = e {
            out.entry(rule.clone()).or_default().push(entry.clone());
        }
    }
    out
}

/// What of `ctx` differs from `now`, in the order of [`Stale`].
fn stale(ctx: &Context, now: &Current<'_>) -> Vec<Stale> {
    [
        (ctx.source_hash != PromptHash::of(now.prompt), Stale::Prompt),
        (ctx.repos != now.repos, Stale::Repos),
        (ctx.rules != now.rules, Stale::Rules),
        (ctx.instructions != now.instructions, Stale::Reader),
    ]
    .into_iter()
    .filter_map(|(differs, why)| differs.then_some(why))
    .collect()
}
