//! The intent table: the raw `may` lines each kind of request expands to, shipped as
//! `intents.yaml` and checked whole when it loads. `docs/specs/prompt-reading.md`, "The table".

use std::collections::BTreeMap;
use std::fmt;
use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

use super::glob;
use crate::config::proxy::{check_allow_entry, GRAPHQL_ENTRY};
use crate::config::{check, parse_strict};

#[cfg(test)]
mod tests;

/// The table as it ships, raw: what the `#/intents` page shows.
pub const INTENTS_TEXT: &str = include_str!("intents.yaml");

/// The longest `words` a row may have, so the reader's list stays one short line a row.
const WORDS_MAX: usize = 120;

/// The repo examples bind `{repo}` to.
const EXAMPLE_REPO: &str = "acme/web";

/// The table, parsed and checked once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntentTable {
    /// The file's `version`.
    pub version: TableVersion,
    /// The rows, in file order.
    pub intents: Vec<Intent>,
}

/// The table's `version`: every change to the file bumps it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TableVersion(pub u32);

/// A row's name, or a reader's own name for a request no row has.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IntentName(String);

impl IntentName {
    /// `text` when it is lower case letters and digits, words joined by one `-`.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let ok = text.split('-').all(|w| {
            !w.is_empty()
                && w.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        });
        ok.then(|| Self(text.to_owned()))
    }

    /// The name as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for IntentName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Intent {
    /// Its name.
    pub name: IntentName,
    /// One line, generated into the reader's instructions.
    pub words: String,
    /// Its `api.github.com` lines may go out as the App.
    pub app: bool,
    /// The lines a `do` clause adds, all together or none.
    pub lines: Vec<RowLine>,
    /// The requests a `never` clause wants refused.
    pub never: Vec<Request>,
}

/// A raw `proxy.allow` entry under the host it goes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowLine {
    /// The host.
    pub host: String,
    /// The entry, with `{repo}`.
    pub entry: String,
}

/// A request a Box could make, as the rules judge it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Request {
    /// One HTTP call, its path a shape such as `/repos/{repo}/pulls/*/merge`.
    Http {
        /// The host.
        host: String,
        /// The method.
        method: String,
        /// The path shape.
        path: String,
    },
    /// A GraphQL mutation, such as `mergePullRequest`.
    Mutation {
        /// The host.
        host: String,
        /// The mutation.
        name: String,
    },
}

impl Request {
    /// The host it goes to.
    #[must_use]
    pub fn host(&self) -> &str {
        match self {
            Self::Http { host, .. } | Self::Mutation { host, .. } => host,
        }
    }
}

/// As the table writes it under its host: `METHOD /path`, or `POST /graphql mutation NAME`.
impl fmt::Display for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Http { method, path, .. } => write!(f, "{method} {path}"),
            Self::Mutation { name, .. } => write!(f, "{GRAPHQL_ENTRY} mutation {name}"),
        }
    }
}

/// Why the table does not load, naming the first bad row.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("intents.yaml{}: {why}", .row.as_ref().map(|r| format!(", row `{r}`")).unwrap_or_default())]
pub struct TableError {
    /// The row, by name or place.
    pub row: Option<String>,
    /// What is wrong.
    pub why: String,
}

/// The file as written.
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTable {
    /// See [`IntentTable::version`].
    version: u32,
    /// The rows.
    intents: Vec<RawIntent>,
}

/// A row as written.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawIntent {
    /// See [`Intent::name`].
    name: String,
    /// See [`Intent::words`].
    words: String,
    /// See [`Intent::app`].
    #[serde(default)]
    app: bool,
    /// Lines by host.
    #[serde(default, rename = "do")]
    lines: BTreeMap<String, Vec<String>>,
    /// Refused requests by host.
    #[serde(default)]
    never: BTreeMap<String, Vec<String>>,
    /// Requests the `do` lines must and must not let through.
    #[serde(default)]
    examples: RawExamples,
}

/// A row's examples, each `METHOD https://host/path`.
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawExamples {
    /// Must pass a `do` line.
    #[serde(default, rename = "match")]
    must: Vec<String>,
    /// Must pass none.
    #[serde(default)]
    no_match: Vec<String>,
}

/// The shipped table, loaded on first use.
static SHIPPED: LazyLock<Result<IntentTable, TableError>> =
    LazyLock::new(|| IntentTable::load(INTENTS_TEXT));

impl IntentTable {
    /// The table moor ships; CI loads the same file, so a running moor never meets an error.
    ///
    /// # Errors
    /// The [`TableError`] of a bad shipped file.
    pub fn shipped() -> Result<&'static Self, &'static TableError> {
        SHIPPED.as_ref()
    }

    /// Parses `text` and runs every row's examples; refuses it whole, naming the first bad row.
    ///
    /// # Errors
    /// [`TableError`] naming the row and what is wrong with it.
    pub fn load(text: &str) -> Result<Self, TableError> {
        let raw: RawTable = parse_strict("intents.yaml", text).map_err(|e| TableError {
            row: None,
            why: e.to_string(),
        })?;
        if raw.version == 0 {
            return Err(TableError {
                row: None,
                why: "version must be 1 or more".into(),
            });
        }
        let mut intents: Vec<Intent> = Vec::with_capacity(raw.intents.len());
        for (at, r) in raw.intents.into_iter().enumerate() {
            let row = if r.name.is_empty() {
                format!("intents[{at}]")
            } else {
                r.name.clone()
            };
            let fail = |why: String| TableError {
                row: Some(row.clone()),
                why,
            };
            let intent = row_of(r).map_err(fail)?;
            if intents.iter().any(|i| i.name == intent.name) {
                return Err(fail("the name is used by an earlier row".into()));
            }
            intents.push(intent);
        }
        Ok(Self {
            version: TableVersion(raw.version),
            intents,
        })
    }

    /// The row named `name`.
    #[must_use]
    pub fn row(&self, name: &IntentName) -> Option<&Intent> {
        self.intents.iter().find(|i| &i.name == name)
    }

    /// One line a row, `name: words`, as the reader's instructions list them.
    #[must_use]
    pub fn list(&self) -> String {
        self.intents
            .iter()
            .map(|i| format!("- {}: {}", i.name, i.words))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// The row `r` describes, checked, with its examples run.
fn row_of(r: RawIntent) -> Result<Intent, String> {
    let name = IntentName::parse(&r.name)
        .ok_or_else(|| format!("name {:?} must be lower case words joined by -", r.name))?;
    if r.words.trim().is_empty() || r.words.contains('\n') || r.words.chars().count() > WORDS_MAX {
        return Err(format!(
            "words must be one line of 1 to {WORDS_MAX} characters"
        ));
    }
    let mut lines = Vec::new();
    for (host, entries) in &r.lines {
        check_host(host)?;
        for entry in entries {
            check_line(entry)?;
            lines.push(RowLine {
                host: host.clone(),
                entry: entry.clone(),
            });
        }
    }
    let mut never = Vec::new();
    for (host, entries) in &r.never {
        check_host(host)?;
        for entry in entries {
            never.push(request_of(host, entry)?);
        }
    }
    if lines.is_empty() && never.is_empty() {
        return Err("a row needs do lines or never requests".into());
    }
    if r.app
        && !lines
            .iter()
            .any(|l| l.host == crate::config::proxy::GRAPHQL_HOST)
    {
        return Err("app: true needs an api.github.com line".into());
    }
    for (example, want) in r
        .examples
        .must
        .iter()
        .map(|e| (e, true))
        .chain(r.examples.no_match.iter().map(|e| (e, false)))
    {
        if example_passes(&lines, example)? != want {
            let says = if want {
                "no do line lets it through"
            } else {
                "a do line lets it through"
            };
            return Err(format!("example {example:?}: {says}"));
        }
    }
    Ok(Intent {
        name,
        words: r.words,
        app: r.app,
        lines,
        never,
    })
}

/// A host: an exact domain.
fn check_host(host: &str) -> Result<(), String> {
    check::host_entry(host)?;
    if host.contains('*') || host != host.to_ascii_lowercase() {
        return Err(format!("host {host:?} must be an exact lower-case domain"));
    }
    Ok(())
}

/// A `do` line: a `proxy.allow` entry with one method, and no `**` or bare `*` path.
fn check_line(entry: &str) -> Result<(), String> {
    check_allow_entry(entry)?;
    let (method, path) = entry.split_once(' ').unwrap_or_default();
    if method.contains('|') {
        return Err(format!("{entry:?} must take one method"));
    }
    if path == "/*" {
        return Err(format!("{entry:?} must name a path, not every path"));
    }
    Ok(())
}

/// A `never` request: `METHOD /path`, or `POST /graphql mutation NAME`.
fn request_of(host: &str, entry: &str) -> Result<Request, String> {
    if let Some(name) = entry.strip_prefix("POST /graphql mutation ") {
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(format!("{entry:?} must name one mutation"));
        }
        return Ok(Request::Mutation {
            host: host.to_owned(),
            name: name.to_owned(),
        });
    }
    check_line(entry)?;
    let (method, path) = entry.split_once(' ').unwrap_or_default();
    Ok(Request::Http {
        host: host.to_owned(),
        method: method.to_owned(),
        path: path.to_owned(),
    })
}

/// Whether `example`, `METHOD https://host/path`, passes one of `lines` with `{repo}` bound.
fn example_passes(lines: &[RowLine], example: &str) -> Result<bool, String> {
    let bad = || format!("example {example:?} must be METHOD https://host/path");
    let (method, at) = example.split_once(' ').ok_or_else(bad)?;
    let url = url::Url::parse(at).map_err(|_| bad())?;
    let host = url.host_str().ok_or_else(bad)?;
    Ok(lines
        .iter()
        .any(|l| l.host == host && glob::lets(&l.entry, EXAMPLE_REPO, method, url.path())))
}
