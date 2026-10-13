//! Pure gate for the per-task proxy: authenticates a request, selects a route and plans the forward.

use crate::secret::Secret;

/// Headers removed from every forwarded request, lowercase.
pub const ALWAYS_STRIP: &[&str] = &[
    "authorization",
    "x-api-key",
    "host",
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

/// Identifier of a route row.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RouteId(String);

impl RouteId {
    /// Wraps a route identifier.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Returns the identifier text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the identifier of the model route.
    #[must_use]
    pub fn model() -> Self {
        Self::new("model")
    }
}

/// Which methods and paths a route admits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Allow {
    /// Any method and any path under the prefix.
    AnyPath,
    /// Smart-HTTP git fetch and push for the Task repo only.
    GitSmartHttp,
    /// Pull-request list, create and view, plus the repo itself, for the Task repo only.
    PullsApi,
}

/// What the proxy adds to the forwarded request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Inject {
    /// Nothing is added.
    None,
    /// `Authorization: Bearer <credential of this row>`, resolved outside core.
    Bearer,
    /// `Authorization: Basic base64(<user>:<credential of this row>)`, resolved outside core.
    Basic {
        /// User name sent with the credential as the password.
        user: &'static str,
    },
}

/// One row of the route table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteRow {
    /// Route identifier.
    pub id: RouteId,
    /// Path prefix without a trailing slash, except `/` which matches everything.
    pub prefix: String,
    /// Method and path admission rule.
    pub allow: Allow,
    /// Extra header names to strip, compared case-insensitively.
    pub strip: Vec<String>,
    /// Credential injection for this route.
    pub inject: Inject,
}

impl RouteRow {
    /// Returns the catch-all model route.
    #[must_use]
    pub fn model() -> Self {
        Self {
            id: RouteId::model(),
            prefix: "/".to_owned(),
            allow: Allow::AnyPath,
            strip: Vec::new(),
            inject: Inject::Bearer,
        }
    }
}

/// Ordered route rows; the longest matching prefix wins.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RouteTable(Vec<RouteRow>);

impl RouteTable {
    /// Builds a table from rows.
    #[must_use]
    pub const fn new(rows: Vec<RouteRow>) -> Self {
        Self(rows)
    }

    /// Returns the rows.
    #[must_use]
    pub fn rows(&self) -> &[RouteRow] {
        &self.0
    }
}

/// A parsed request head to be judged.
pub struct GateInput<'a> {
    /// Route rows to select from.
    pub table: &'a RouteTable,
    /// Request method.
    pub method: &'a str,
    /// Request target: path plus optional `?query`.
    pub target: &'a str,
    /// Header name and value pairs; names in any case.
    pub headers: &'a [(&'a str, &'a str)],
    /// Expected nonce as base64url text bytes.
    pub nonce: &'a Secret,
    /// Forge repo of the Task as `owner/name`, when it has one.
    pub task_repo: Option<&'a str>,
}

/// How to forward an admitted request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// Selected route.
    pub route: RouteId,
    /// Method to send upstream.
    pub method: String,
    /// Authenticated path plus `?query` when one was present.
    pub upstream_path: String,
    /// Headers to send upstream, original case and order.
    pub headers: Vec<(String, String)>,
    /// Credential injection to apply.
    pub inject: Inject,
}

/// Why a request is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reject {
    /// Malformed request target (400).
    BadRequest,
    /// Missing or wrong nonce (401).
    Unauthorized,
    /// Authenticated but not admitted, with a reason (403).
    Forbidden(&'static str),
    /// No route matches the path (404).
    NotFound,
    /// Method not admitted by the route (405).
    BadMethod,
}

impl Reject {
    /// Returns the HTTP status code.
    #[must_use]
    pub const fn status(self) -> u16 {
        match self {
            Self::BadRequest => 400,
            Self::Unauthorized => 401,
            Self::Forbidden(_) => 403,
            Self::NotFound => 404,
            Self::BadMethod => 405,
        }
    }
}

/// Outcome of the gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateDecision {
    /// Forward the request as planned.
    Forward(Plan),
    /// Refuse the request.
    Reject(Reject),
}

/// Compares two byte strings without an early exit on content; length is not secret.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let acc = a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y));
    std::hint::black_box(acc) == 0
}

/// Checks an offered nonce against the expected one; an empty expected nonce never matches.
fn nonce_ok(expected: &Secret, offered: &str) -> bool {
    let expected = expected.expose();
    !expected.is_empty() && ct_eq(expected, offered.as_bytes())
}

/// Authenticates and returns the path with any nonce segment removed.
fn authenticate(input: &GateInput<'_>, path: &str) -> Result<String, Reject> {
    if let Some(rest) = path.strip_prefix("/t/") {
        let (offered, tail) = rest.split_once('/').unwrap_or((rest, ""));
        if offered.is_empty() || !nonce_ok(input.nonce, offered) {
            return Err(Reject::Unauthorized);
        }
        return Ok(format!("/{tail}"));
    }
    let value = input
        .headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("authorization"))
        .map(|(_, value)| *value);
    let token = value
        .and_then(|v| v.split_once(' '))
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("Bearer"))
        .map(|(_, token)| token);
    match token {
        Some(token) if nonce_ok(input.nonce, token) => Ok(path.to_owned()),
        _ => Err(Reject::Unauthorized),
    }
}

/// Returns the normalised prefix length when the row matches the path.
fn match_len(row: &RouteRow, path: &str) -> Option<usize> {
    let prefix = row.prefix.trim_end_matches('/');
    let hit = prefix.is_empty()
        || path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('/'));
    hit.then_some(prefix.len())
}

/// Returns whether a header must not be forwarded.
fn stripped(row: &RouteRow, connection: &[String], name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    ALWAYS_STRIP.contains(&lower.as_str())
        || row.strip.iter().any(|s| s.eq_ignore_ascii_case(&lower))
        || connection.contains(&lower)
}

/// Judges a request: authenticate, select a route, and plan the forward.
#[must_use]
pub fn decide(input: &GateInput<'_>) -> GateDecision {
    match plan(input) {
        Ok(plan) => GateDecision::Forward(plan),
        Err(reject) => GateDecision::Reject(reject),
    }
}

/// Builds the forward plan or the refusal.
fn plan(input: &GateInput<'_>) -> Result<Plan, Reject> {
    if !input.target.starts_with('/') {
        return Err(Reject::BadRequest);
    }
    let (raw_path, query) = match input.target.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (input.target, None),
    };
    let path = authenticate(input, raw_path)?;
    let (len, row) = input
        .table
        .rows()
        .iter()
        .filter_map(|row| match_len(row, &path).map(|len| (len, row)))
        .fold(None::<(usize, &RouteRow)>, |best, cand| match best {
            Some((len, _)) if len >= cand.0 => best,
            _ => Some(cand),
        })
        .ok_or(Reject::NotFound)?;
    let rest = path.get(len..).unwrap_or_default();
    match row.allow {
        Allow::AnyPath => {}
        Allow::GitSmartHttp => forge::admit_git(input.method, rest, query, input.task_repo)?,
        Allow::PullsApi => forge::admit_api(input.method, rest, query, input.task_repo)?,
    }
    let connection: Vec<String> = input
        .headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("connection"))
        .flat_map(|(_, value)| value.split(','))
        .map(|token| token.trim().to_ascii_lowercase())
        .collect();
    let headers = input
        .headers
        .iter()
        .filter(|(name, _)| !stripped(row, &connection, name))
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect();
    let upstream_path = query.map_or_else(|| path.clone(), |q| format!("{path}?{q}"));
    Ok(Plan {
        route: row.id.clone(),
        method: input.method.to_owned(),
        upstream_path,
        headers,
        inject: row.inject,
    })
}

mod forge;

#[cfg(test)]
mod tests;
