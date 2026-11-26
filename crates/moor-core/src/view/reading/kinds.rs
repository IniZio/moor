//! The closed words of a `ReadingView`: where it stands, why it is stale, a phrase's mark, and an
//! effect's kind and origin.

use serde::{Deserialize, Serialize};

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
    /// A newer intent table gives the reading other lines, or a row for a clause it lacked.
    Table,
}

/// The underline a phrase draws: solid or dashed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarkView {
    /// Solid: moor holds the agent to it.
    Held,
    /// Dashed: only asked.
    Asked,
    /// "moor has no rule for this": holds nothing.
    Unmapped,
}

/// Where an allow effect came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FromView {
    /// Equal to a line of the config's `proxy.allow`, from slice 1.
    Listed,
    /// A row of the intent table.
    Table,
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
