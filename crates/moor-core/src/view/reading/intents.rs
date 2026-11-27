//! The parts of a `ReadingView` intents add: each phrase's intents, the config's hand-written
//! `may` lines tagged against the reading, and whether a newer table changes the reading.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

#[cfg(test)]
mod tests;

use crate::config::proxy::{ToolRule, APP_CREDENTIAL};
use crate::reading::{
    lets_through, Effect, Found, IntentTable, IntentUse, Line, Mark, Polarity, ProxyAllow, Read,
    SendAs, TableVersion, Unholdable,
};

/// One intent a phrase names, as the hover shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntentView {
    /// The intent's name.
    pub intent: String,
    /// Asked for or forbidden.
    pub polarity: Polarity,
    /// The repo the words name; none for each repo of the Box.
    pub repo: Option<String>,
    /// Who the write goes out as.
    #[serde(rename = "as")]
    pub send_as: SendAs,
    /// What expansion found: `lines`, `cannot_hold` or `no_row`.
    pub found: String,
    /// Why moor cannot hold it, when it cannot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unholdable: Option<Unholdable>,
    /// The row's lines the pinned rules already let through, raw.
    pub already: Vec<String>,
    /// Whether the table moor runs has a row of this name.
    pub row: bool,
}

/// What a hand-written line is against the reading's lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tag {
    /// An allow effect has the same rule and entry.
    Reproduced,
    /// The pinned rules let it through without it.
    AlreadyAllowed,
    /// An allow effect has the entry under the other GitHub credential.
    OtherCredential,
    /// Anything else.
    Missed,
}

/// One line of the config's `proxy.allow`, tagged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandwrittenView {
    /// The rule it is listed under.
    pub rule: String,
    /// The entry, raw.
    pub line: String,
    /// What it is against the reading.
    pub tag: Tag,
}

/// `u` as the hover shows it, against the current `table`.
pub(super) fn intent(u: &IntentUse, table: Option<&IntentTable>) -> IntentView {
    let (found, unholdable, already) = match &u.found {
        Found::Lines { already } => (
            "lines",
            None,
            already.iter().map(|l| l.entry.clone()).collect(),
        ),
        Found::CannotHold { why, .. } => ("cannot_hold", Some(*why), Vec::new()),
        Found::NoRow => ("no_row", None, Vec::new()),
        Found::Refused => ("refused", None, Vec::new()),
        Found::LetThrough(_) => ("let_through", None, Vec::new()),
    };
    IntentView {
        intent: u.intent.to_string(),
        polarity: u.polarity,
        repo: u.repo.as_ref().map(ToString::to_string),
        send_as: u.send_as,
        found: found.to_owned(),
        unholdable,
        already,
        row: table.is_some_and(|t| t.row(&u.intent).is_some()),
    }
}

/// Each line of `allow` tagged against the allow effects of `read` under `rules`.
pub(super) fn handwritten(
    read: &Read,
    allow: &ProxyAllow,
    rules: &BTreeMap<String, ToolRule>,
) -> Vec<HandwrittenView> {
    let derived: Vec<Line> = read
        .effects()
        .iter()
        .filter_map(|e| match e {
            Effect::Allow { rule, entry, .. } => Some(Line {
                rule: rule.clone(),
                entry: entry.clone(),
            }),
            _ => None,
        })
        .collect();
    allow
        .iter()
        .flat_map(|(rule, entries)| entries.iter().map(move |e| (rule, e)))
        .map(|(rule, entry)| {
            let same = |l: &&Line| l.entry == *entry;
            let other =
                |l: &&Line| l.rule != *rule && (l.rule == APP_CREDENTIAL || rule == APP_CREDENTIAL);
            let tag = if derived.iter().filter(same).any(|l| l.rule == *rule) {
                Tag::Reproduced
            } else if lets_through(rules, rule, entry) {
                Tag::AlreadyAllowed
            } else if derived.iter().filter(same).any(|l| other(&l)) {
                Tag::OtherCredential
            } else {
                Tag::Missed
            };
            HandwrittenView {
                rule: rule.clone(),
                line: entry.clone(),
                tag,
            }
        })
        .collect()
}

/// Whether `table`, newer than `read_with`, changes `read`: a clause no row covered, or a
/// phrase whose rows now give other entries than it holds.
pub(super) fn table_changes(read: &Read, read_with: TableVersion, table: &IntentTable) -> bool {
    if read_with == table.version {
        return false;
    }
    let entries: BTreeMap<String, &str> = read
        .effects()
        .iter()
        .filter_map(|e| match e {
            Effect::Allow { id, entry, .. } => Some((id.to_string(), entry.as_str())),
            _ => None,
        })
        .collect();
    read.phrases().iter().any(|p| {
        if p.intents.iter().any(|u| u.found == Found::NoRow) {
            return true;
        }
        let held: BTreeSet<&str> = match &p.mark {
            Mark::Held { effects } => effects
                .iter()
                .filter_map(|e| entries.get(&e.to_string()).copied())
                .collect(),
            Mark::Asked { .. } | Mark::Unmapped {} => BTreeSet::new(),
        };
        let mut had = held;
        let mut now = BTreeSet::new();
        for u in p.intents.iter().filter(|u| u.polarity == Polarity::Do) {
            if let Found::Lines { already } = &u.found {
                had.extend(already.iter().map(|l| l.entry.as_str()));
            }
            let named = u.repo.as_ref().map(ToString::to_string);
            let Some(row) = table.row(&u.intent) else {
                return true;
            };
            now.extend(row.lines.iter().map(|l| {
                named
                    .as_ref()
                    .map_or_else(|| l.entry.clone(), |r| l.entry.replace("{repo}", r))
            }));
        }
        let now: BTreeSet<&str> = now.iter().map(String::as_str).collect();
        p.intents.iter().any(|u| u.polarity == Polarity::Do) && had != now
    })
}
