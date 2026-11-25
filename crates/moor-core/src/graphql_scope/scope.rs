//! Which GraphQL root fields and mutations a rule lets through, as its `POST /graphql` entry
//! lists them.
//!
//! Each query root keeps its own check in code (`repository` must name the Box repo, `node`
//! results read as `PullRequest`, and so on), so only those roots may be listed. A mutation
//! names the input keys whose node ids must sit in the Box repos: the first is required, the
//! rest are checked when given.

use serde::{Deserialize, Serialize};

/// The query roots the policy knows how to check.
pub const ROOTS: &[&str] = &["repository", "viewer", "node", "nodes", "search", "__type"];

/// A mutation and the input keys that hold node ids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mutation {
    /// The mutation field name.
    pub name: String,
    /// Input keys holding node ids; the first is required.
    pub ids: Vec<String>,
}

/// The roots and mutations a rule lets through.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ScopeText", into = "ScopeText")]
pub struct Scope {
    /// Allowed query roots, each one of [`ROOTS`].
    pub query: Vec<String>,
    /// Allowed mutations.
    pub mutation: Vec<Mutation>,
}

/// The scope as the rules text writes it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
struct ScopeText {
    /// Query root names.
    query: Vec<String>,
    /// `name(idKey, ...)` entries.
    mutation: Vec<String>,
}

impl Scope {
    /// The keys of mutation `name`, when it is listed.
    #[must_use]
    pub fn mutation(&self, name: &str) -> Option<&[String]> {
        self.mutation
            .iter()
            .find(|m| m.name == name)
            .map(|m| m.ids.as_slice())
    }

    /// Whether query root `name` is listed.
    #[must_use]
    pub fn allows_root(&self, name: &str) -> bool {
        self.query.iter().any(|q| q == name)
    }
}

/// Whether `s` is a GraphQL name.
fn is_name(s: &str) -> bool {
    let mut chars = s.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Parses `name(idKey, ...)`.
fn parse_mutation(entry: &str) -> Result<Mutation, String> {
    let bad = || {
        format!(
            "expected \"mutationName(idKey, ...)\" naming the input keys whose ids must be in the Box repo, got {entry:?}"
        )
    };
    let (name, rest) = entry.trim().split_once('(').ok_or_else(bad)?;
    let keys = rest.strip_suffix(')').ok_or_else(bad)?;
    let ids: Vec<String> = keys.split(',').map(|k| k.trim().to_owned()).collect();
    let name = name.trim();
    if !is_name(name) || ids.iter().any(|k| !is_name(k)) {
        return Err(bad());
    }
    Ok(Mutation {
        name: name.to_owned(),
        ids,
    })
}

impl TryFrom<ScopeText> for Scope {
    type Error = String;

    fn try_from(text: ScopeText) -> Result<Self, String> {
        for root in &text.query {
            if !ROOTS.contains(&root.as_str()) {
                return Err(format!(
                    "query root {root:?} has no check in moor; known roots are {}",
                    ROOTS.join(", ")
                ));
            }
        }
        let mutation = text
            .mutation
            .iter()
            .map(|m| parse_mutation(m))
            .collect::<Result<_, _>>()?;
        Ok(Self {
            query: text.query,
            mutation,
        })
    }
}

#[cfg(test)]
mod tests;

impl From<Scope> for ScopeText {
    fn from(scope: Scope) -> Self {
        Self {
            query: scope.query,
            mutation: scope
                .mutation
                .into_iter()
                .map(|m| format!("{}({})", m.name, m.ids.join(", ")))
                .collect(),
        }
    }
}
