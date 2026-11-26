//! moor's reading of a prompt: the words it holds the agent to, the words it only asks, and the
//! gates and allow entries those words produce. `docs/specs/prompt-reading.md` is the model.
//!
//! A [`Read`] is built only by [`Reading::hand_in`], which places every quote and checks every
//! effect, by [`Read::from_gates`] for gates stored without a reading, or by parsing JSON that
//! keeps the same rules.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::name::{BoxName, ForgeRepo};
use crate::seam::clock::Ts;
use crate::user::UserId;
use crate::workflow::{PromptHash, WorkflowId};

mod effect;
mod expand;
mod glob;
mod hand_in;
mod id;
mod intent;
mod place;
mod read;
mod refusal;
mod table;
#[cfg(test)]
mod tests;

pub use effect::{Change, Comparison, Effect, EffectKey, Origin};
pub use expand::{lets_through, Pinned};
pub use hand_in::{EffectDoc, HandIn, IntentDoc, PhraseDoc};
pub use id::{EffectId, IdError, PhraseId, ReadingId};
pub use intent::{Found, IntentUse, Line, Polarity, SendAs, Unholdable};
pub use read::{EffectIds, Mark, Phrase, Quote, Read, Run, Span};
pub use refusal::Refusal;
pub use table::{
    Intent, IntentName, IntentTable, Request, RowLine, TableError, TableVersion, INTENTS_TEXT,
};

/// The workflow config's `proxy.allow`: rule name to its extra entries.
pub type ProxyAllow = BTreeMap<String, Vec<String>>;

/// One read of one source text, against the repos and rules it was read with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reading {
    /// Its id.
    pub id: ReadingId,
    /// What it reads.
    pub of: ReadingOf,
    /// The reading this one answers or reads again.
    pub parent: Option<ReadingId>,
    /// The words read, verbatim.
    pub source: String,
    /// What else the reader saw.
    pub context: Context,
    /// Whose token paid.
    pub made_by: UserId,
    /// Where the job that makes it stands.
    pub state: ReadingState,
}

impl Reading {
    /// The read `doc` hands in for this reading's source, with moor's ids, when every quote
    /// places and every effect checks; each intent is expanded by `table` under `pinned`.
    ///
    /// # Errors
    /// [`Refusal`] naming the first phrase or effect that does not hold.
    pub fn hand_in(
        &self,
        doc: &HandIn,
        table: &IntentTable,
        pinned: &Pinned<'_>,
    ) -> Result<Read, Refusal> {
        let ReadingState::Reading { by, .. } = &self.state else {
            return Err(Refusal::NotReading);
        };
        let against = hand_in::Against {
            repos: &self.context.repos,
            table,
            pinned,
        };
        hand_in::check(doc, &self.source, &against, by.clone())
    }
}

/// What a reading reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadingOf {
    /// An automation's saved or drafted prompt.
    Automation(WorkflowId),
}

/// What else the reader saw. A reading is stale when any of it differs now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Context {
    /// The hash of the source.
    #[serde(with = "prompt_hash")]
    pub source_hash: PromptHash,
    /// The repos, in order; the first is where the agent works.
    pub repos: Vec<ForgeRepo>,
    /// The rules texts the Box follows: the first repo's today.
    pub rules: Vec<RulesPin>,
    /// The version of the reader's instructions.
    pub instructions: u32,
    /// The intent table it was expanded with; none before intents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table: Option<TableVersion>,
}

/// One rules text a reading was read against.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RulesPin {
    /// The repo whose rules; none for the default rules.
    pub repo: Option<ForgeRepo>,
    /// The rules revision.
    pub revision: u64,
}

impl RulesPin {
    /// The default rules: no stored row, so no save changes their revision.
    pub const DEFAULT: Self = Self {
        repo: None,
        revision: 0,
    };
}

/// Where the job that makes a reading stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadingState {
    /// A reader is reading.
    Reading {
        /// Who reads it.
        by: ReaderRun,
        /// Since when.
        since: Ts,
    },
    /// No read came.
    Failed {
        /// Why, in words a person reads.
        why: String,
        /// When it failed.
        at: Ts,
    },
    /// Read.
    Read(Read),
}

/// Who read it. Data only: moor never branches on the harness or the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReaderRun {
    /// The reader's Box.
    pub box_name: BoxName,
    /// The agent harness.
    pub harness: String,
    /// The model, when the harness names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// [`PromptHash`] on the wire, as 64 hex digits.
mod prompt_hash {
    use serde::{de::Error as _, Deserialize as _, Deserializer, Serializer};

    use crate::workflow::PromptHash;

    /// Writes the hash as hex.
    pub(super) fn serialize<S: Serializer>(hash: &PromptHash, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hash.hex())
    }

    /// Reads the hash from hex.
    pub(super) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<PromptHash, D::Error> {
        let hex = String::deserialize(d)?;
        PromptHash::from_hex(&hex).ok_or_else(|| D::Error::custom("not a 64 hex digit hash"))
    }
}
