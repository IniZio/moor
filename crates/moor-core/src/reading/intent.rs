//! What a phrase's words ask for or forbid, and what expansion found for each.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{IntentName, Request};
use crate::name::ForgeRepo;

/// One intent a phrase names, and what expansion found for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntentUse {
    /// A row's name, or the reader's own when no row has it.
    pub intent: IntentName,
    /// Whether the words ask for the request or forbid it.
    pub polarity: Polarity,
    /// The repo the words name; none for each repo of the Box.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<ForgeRepo>,
    /// Who the write goes out as.
    #[serde(rename = "as", default)]
    pub send_as: SendAs,
    /// What expansion found, kept so a page needs no old table.
    pub found: Found,
}

/// Whether a clause asks for a request or forbids it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Polarity {
    /// The words ask for it.
    Do,
    /// The words forbid it.
    Never,
}

/// Who a write goes out as.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SendAs {
    /// The person's own token.
    #[default]
    Person,
    /// The GitHub App, only when the words ask for it.
    App,
}

/// What expansion found for one intent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Found {
    /// A `do` row: its other lines are the phrase's allow effects.
    Lines {
        /// The row's lines the pinned rules already let through.
        already: Vec<Line>,
    },
    /// A `never` row: the pinned rules refuse every request it lists; they are withheld effects.
    Refused,
    /// A `never` row whose request the pinned rules let through, so the clause holds nothing.
    LetThrough(Allowed),
    /// A line of the row moor cannot hold, so the clause holds nothing.
    CannotHold {
        /// The line.
        line: Line,
        /// Why.
        why: Unholdable,
    },
    /// No row moor holds has it: "moor has no rule for this".
    NoRow,
}

/// Why moor cannot hold a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unholdable {
    /// The pinned rules have no rule for its host.
    NoRuleForHost,
    /// It would go out as the App, and this controller has none.
    NoApp,
}

/// A raw `may` line: a `proxy.allow` entry under the rule that takes it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Line {
    /// The rule, as `proxy.allow` keys it.
    pub rule: String,
    /// The entry, raw.
    pub entry: String,
}

/// A forbidding clause moor cannot hold: the pinned rules, or the reading's own lines, let
/// `request` through under `rule`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Allowed {
    /// The request let through.
    pub request: Request,
    /// The rule that lets it through, as `proxy.allow` keys it.
    pub rule: String,
}
