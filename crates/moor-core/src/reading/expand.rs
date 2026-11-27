//! Expansion: each intent a reader names becomes the raw lines of its row, under the pinned rule
//! for each line's host, with the lines those rules already let through set apart.

use std::collections::BTreeMap;

use super::glob::covers;
use super::hand_in::IntentDoc;
use super::{
    Found, IntentName, IntentTable, IntentUse, Line, Polarity, Refusal, RowLine, SendAs, Unholdable,
};
use crate::config::proxy::{ToolRule, APP_CREDENTIAL, GRAPHQL_HOST};
use crate::name::ForgeRepo;

#[cfg(test)]
mod tests;

/// The rules a reading is checked against: the rules text the Box follows, by rule id.
#[derive(Debug, Clone, Copy)]
pub struct Pinned<'a> {
    /// The pinned rules text, by rule id.
    pub rules: &'a BTreeMap<String, ToolRule>,
    /// Whether this controller has a GitHub App to send `app` lines as.
    pub app: bool,
}

/// `doc`, a clause of the phrase quoting `quote`, as moor holds it, with the lines it adds.
pub(super) fn expand(
    doc: &IntentDoc,
    quote: &str,
    table: &IntentTable,
    pinned: &Pinned<'_>,
    repos: &[ForgeRepo],
) -> Result<(IntentUse, Vec<Line>), Refusal> {
    let refused_name = || Refusal::IntentName {
        quote: quote.to_owned(),
        intent: doc.intent.clone(),
    };
    let intent = IntentName::parse(&doc.intent).ok_or_else(refused_name)?;
    let repo = match &doc.repo {
        None => None,
        Some(named) => Some(
            repos
                .iter()
                .find(|r| r.to_string() == *named)
                .cloned()
                .ok_or_else(|| Refusal::UnknownRepo {
                    quote: quote.to_owned(),
                    repo: named.clone(),
                })?,
        ),
    };
    let row = table.row(&intent);
    if doc.send_as == SendAs::App && row.is_some_and(|r| !r.app) {
        return Err(Refusal::NoAppRow {
            quote: quote.to_owned(),
            intent: doc.intent.clone(),
        });
    }
    let held = |found| IntentUse {
        intent: intent.clone(),
        polarity: doc.polarity,
        repo: repo.clone(),
        send_as: doc.send_as,
        found,
    };
    // Forbidding words are held from slice 3; until then the clause holds nothing.
    let Some(row) = row.filter(|_| doc.polarity == Polarity::Do) else {
        return Ok((held(Found::NoRow), Vec::new()));
    };
    let named = repo.as_ref().map(ToString::to_string);
    let (mut already, mut added) = (Vec::new(), Vec::new());
    for l in &row.lines {
        let app = doc.send_as == SendAs::App && l.host == GRAPHQL_HOST;
        match line_of(l, named.as_deref(), app, pinned) {
            Ok((line, true)) => already.push(line),
            Ok((line, false)) => added.push(line),
            Err((line, why)) => return Ok((held(Found::CannotHold { line, why }), Vec::new())),
        }
    }
    Ok((held(Found::Lines { already }), added))
}

/// The row line `l` under the pinned rule for its host, with whether that rule already lets it
/// through; or the line and why moor cannot hold it. `app` sends it as the App.
fn line_of(
    l: &RowLine,
    repo: Option<&str>,
    app: bool,
    pinned: &Pinned<'_>,
) -> Result<(Line, bool), (Line, Unholdable)> {
    let entry = repo.map_or_else(|| l.entry.clone(), |r| l.entry.replace("{repo}", r));
    let cannot = |why| {
        let rule = if app { APP_CREDENTIAL } else { "" };
        let line = Line {
            rule: rule.to_owned(),
            entry: entry.clone(),
        };
        Err((line, why))
    };
    let serves = |r: &&ToolRule| r.hosts.iter().any(|h| h.eq_ignore_ascii_case(&l.host));
    let Some((id, rule)) = pinned.rules.iter().find(|(_, r)| serves(r)) else {
        return cannot(Unholdable::NoRuleForHost);
    };
    let (rule, covering) = if app {
        if !pinned.app {
            return cannot(Unholdable::NoApp);
        }
        (APP_CREDENTIAL.to_owned(), lines_of(&rule.app, false))
    } else {
        (id.clone(), lines_of(&rule.allow, true))
    };
    let already = covering.iter().any(|c| covers(c, &entry, repo));
    Ok((Line { rule, entry }, already))
}

/// The entries of a rule that let requests through; a rule with no path list lets all through.
fn lines_of(entries: &[String], open_when_empty: bool) -> Vec<String> {
    if entries.is_empty() && open_when_empty {
        return vec!["*".to_owned()];
    }
    entries.to_vec()
}
