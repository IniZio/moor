//! Expansion: each intent a reader names becomes the raw lines of its row, under the pinned rule
//! for each line's host, with the lines those rules already let through set apart.

use std::collections::BTreeMap;

use super::glob::covers;
use super::hand_in::IntentDoc;
use super::{
    Found, IntentName, IntentTable, IntentUse, Line, Polarity, Refusal, Request, RowLine, SendAs,
    Unholdable,
};
use crate::config::proxy::{ToolRule, APP_CREDENTIAL, GRAPHQL_HOST};
use crate::name::ForgeRepo;

#[cfg(test)]
mod tests;

/// Where a row's line or request names the Box's repo.
const REPO_SLOT: &str = "{repo}";

/// The rules a reading is checked against: the rules text the Box follows, by rule id.
#[derive(Debug, Clone, Copy)]
pub struct Pinned<'a> {
    /// The pinned rules text, by rule id.
    pub rules: &'a BTreeMap<String, ToolRule>,
    /// Whether this controller has a GitHub App to send `app` lines as.
    pub app: bool,
}

/// One clause expanded: what moor holds of it, the lines it adds, and the requests it forbids.
pub(super) struct Expanded {
    /// The clause as moor holds it; a `never` clause reads [`Found::Refused`] until its
    /// requests are checked against the whole reading's lines.
    pub(super) used: IntentUse,
    /// The lines a `do` clause adds.
    pub(super) adds: Vec<Line>,
    /// The requests a `never` clause wants refused, with the named repo written in.
    pub(super) forbids: Vec<Request>,
}

/// `doc`, a clause of the phrase quoting `quote`, as moor holds it, with the lines it adds or
/// the requests it forbids.
pub(super) fn expand(
    doc: &IntentDoc,
    quote: &str,
    table: &IntentTable,
    pinned: &Pinned<'_>,
    repos: &[ForgeRepo],
) -> Result<Expanded, Refusal> {
    let refused_name = || Refusal::IntentName {
        quote: quote.to_owned(),
        intent: doc.intent.clone(),
    };
    let intent = IntentName::parse(&doc.intent).ok_or_else(refused_name)?;
    let repo = repo_of(doc, quote, repos)?;
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
    let only = |found| Expanded {
        used: held(found),
        adds: Vec::new(),
        forbids: Vec::new(),
    };
    let named = repo.as_ref().map(ToString::to_string);
    let Some(row) = row else {
        return Ok(only(Found::NoRow));
    };
    if doc.polarity == Polarity::Never {
        let forbids: Vec<Request> = row
            .never
            .iter()
            .map(|r| with_repo(r, named.as_deref()))
            .collect();
        let found = if forbids.is_empty() {
            Found::NoRow
        } else {
            Found::Refused
        };
        return Ok(Expanded {
            forbids,
            ..only(found)
        });
    }
    let (mut already, mut added) = (Vec::new(), Vec::new());
    for l in &row.lines {
        let app = doc.send_as == SendAs::App && l.host == GRAPHQL_HOST;
        match line_of(l, named.as_deref(), app, pinned) {
            Ok((line, true)) => already.push(line),
            Ok((line, false)) => added.push(line),
            Err((line, why)) => return Ok(only(Found::CannotHold { line, why })),
        }
    }
    Ok(Expanded {
        adds: added,
        ..only(Found::Lines { already })
    })
}

/// The repo `doc` names, one of `repos`; none when it names none.
fn repo_of(
    doc: &IntentDoc,
    quote: &str,
    repos: &[ForgeRepo],
) -> Result<Option<ForgeRepo>, Refusal> {
    let Some(named) = &doc.repo else {
        return Ok(None);
    };
    repos
        .iter()
        .find(|r| r.to_string() == *named)
        .cloned()
        .map(Some)
        .ok_or_else(|| Refusal::UnknownRepo {
            quote: quote.to_owned(),
            repo: named.clone(),
        })
}

/// `r` with `{repo}` written as `repo`, when the words name one.
fn with_repo(r: &Request, repo: Option<&str>) -> Request {
    match (r, repo) {
        (Request::Http { host, method, path }, Some(repo)) => Request::Http {
            host: host.clone(),
            method: method.clone(),
            path: path.replace(REPO_SLOT, repo),
        },
        _ => r.clone(),
    }
}

/// The row line `l` under the pinned rule for its host, with whether that rule already lets it
/// through; or the line and why moor cannot hold it. `app` sends it as the App.
fn line_of(
    l: &RowLine,
    repo: Option<&str>,
    app: bool,
    pinned: &Pinned<'_>,
) -> Result<(Line, bool), (Line, Unholdable)> {
    let entry = repo.map_or_else(|| l.entry.clone(), |r| l.entry.replace(REPO_SLOT, r));
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

/// Whether `rules` already let `entry` through under `rule`, a rule id or [`APP_CREDENTIAL`]
/// for the App's lines of the rule for the GitHub API.
#[must_use]
pub fn lets_through(rules: &BTreeMap<String, ToolRule>, rule: &str, entry: &str) -> bool {
    let covering = if rule == APP_CREDENTIAL {
        rules
            .values()
            .find(|r| r.hosts.iter().any(|h| h.eq_ignore_ascii_case(GRAPHQL_HOST)))
            .map(|r| lines_of(&r.app, false))
    } else {
        rules.get(rule).map(|r| lines_of(&r.allow, true))
    };
    covering.is_some_and(|c| c.iter().any(|c| covers(c, entry, None)))
}

/// The entries of a rule that let requests through; a rule with no path list lets all through.
fn lines_of(entries: &[String], open_when_empty: bool) -> Vec<String> {
    if entries.is_empty() && open_when_empty {
        return vec!["*".to_owned()];
    }
    entries.to_vec()
}
