//! Scope check for GitHub GraphQL requests: decides from the request alone whether it touches
//! only the Box repo, so a broad user token cannot reach other repos through the proxy.
//!
//! The policy is an allow-list: besides the root rules, every field under a root field must be
//! listed for the kind of object it is selected on, so account data and other repos stay out of
//! reach. The document is parsed strictly. Root fields are judged by their real names, never by alias or
//! operation name, and fragments are expanded before judging.

use serde_json::{Map, Value as Json};

mod lex;
mod parse;
mod policy;
mod schema;
mod strict;

#[cfg(test)]
mod tests;

/// Largest request body that is parsed, in bytes.
const MAX_BODY: usize = 64 * 1024;

/// The repository a request may touch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoRef {
    /// Account or organisation that owns the repo.
    pub owner: String,
    /// Repo name, without the owner.
    pub name: String,
}

impl RepoRef {
    /// Builds a repo reference from an owner and a name.
    #[must_use]
    pub fn new(owner: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            owner: owner.into(),
            name: name.into(),
        }
    }
}

/// A GraphQL node ID that names an object by ID only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeId(String);

impl NodeId {
    /// Returns the ID text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Wraps an ID.
    pub(crate) fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

/// Why a request is refused; the text names the offending field or limit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reason(String);

impl Reason {
    /// Returns the reason text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Wraps a reason. A value not known to be a GraphQL name goes in through [`shown`], so
    /// the only backticks left are the ones the text puts around names and values.
    pub(crate) fn new(text: impl Into<String>) -> Self {
        Self(text.into())
    }

    /// The reason safe to record: a backticked span stays only when it is a GraphQL name
    /// (a field or variable, never a value such as a repo name or search term), and the
    /// text is cut to 160 characters.
    #[must_use]
    pub fn redacted(&self) -> String {
        let mut out = String::new();
        for (i, part) in self.0.split('`').enumerate() {
            if i % 2 == 0 {
                out.push_str(part);
            } else if is_name_path(part) {
                out.push('`');
                out.push_str(part);
                out.push('`');
            } else {
                out.push_str("`…`");
            }
        }
        out.chars()
            .filter(|c| !c.is_control())
            .take(MAX_REDACTED)
            .collect()
    }
}

/// `value` as a reason may quote it: its backticks become `'`, so it cannot close or open a
/// backticked span.
pub(crate) fn shown(value: &str) -> String {
    value.replace('`', "'")
}

/// Longest redacted reason, in characters.
const MAX_REDACTED: usize = 160;
/// Longest backticked name kept in a redacted reason.
const MAX_NAME: usize = 64;

/// A field, variable or dotted field path of at most [`MAX_NAME`] characters, or one
/// punctuation character.
fn is_name_path(s: &str) -> bool {
    let name = s.len() <= MAX_NAME
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'$'));
    let punct = s.len() == 1 && s.bytes().all(|b| b.is_ascii_punctuation());
    !s.is_empty() && (name || punct)
}

/// Outcome of judging a GraphQL request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Every field is confined to the Box repo.
    Allow,
    /// Allowed only if each listed node ID resolves to an object in the Box repo.
    NeedsNodeCheck(Vec<NodeId>),
    /// The request is refused.
    Deny(Reason),
}

/// Judges a GraphQL request body against the Box repo.
#[must_use]
pub fn judge(body: &[u8], repo: &RepoRef) -> Verdict {
    match scope(body, repo) {
        Ok(ids) if ids.is_empty() => Verdict::Allow,
        Ok(ids) => Verdict::NeedsNodeCheck(ids),
        Err(reason) => Verdict::Deny(reason),
    }
}

/// Parses and checks the request, returning the node IDs that still need a check.
fn scope(body: &[u8], repo: &RepoRef) -> Result<Vec<NodeId>, Reason> {
    if body.len() > MAX_BODY {
        return Err(Reason::new("request body over 64 KiB"));
    }
    let (query, vars) = request(body)?;
    let doc = parse::parse(&query)
        .map_err(|err| Reason::new(format!("malformed query: {}", shown(&err))))?;
    policy::run(&doc, &vars, repo)
}

/// Splits the JSON request into query text and variables; `operationName` is ignored.
fn request(body: &[u8]) -> Result<(String, Map<String, Json>), Reason> {
    let strict: strict::Strict = serde_json::from_slice(body).map_err(|err| {
        Reason::new(format!(
            "malformed request json: {}",
            shown(&err.to_string())
        ))
    })?;
    let Json::Object(mut obj) = strict.0 else {
        return Err(Reason::new("request is not a json object"));
    };
    if let Some(key) = obj
        .keys()
        .find(|k| !matches!(k.as_str(), "query" | "variables" | "operationName"))
    {
        return Err(Reason::new(format!(
            "unexpected request key `{}`",
            shown(key)
        )));
    }
    let Some(Json::String(query)) = obj.remove("query") else {
        return Err(Reason::new("request has no query string"));
    };
    let vars = match obj.remove("variables") {
        None | Some(Json::Null) => Map::new(),
        Some(Json::Object(vars)) => vars,
        Some(_) => return Err(Reason::new("variables is not an object")),
    };
    Ok((query, vars))
}
