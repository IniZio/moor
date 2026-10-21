//! Reads a repo's base commit and config files with one GitHub GraphQL request.

use std::sync::Arc;
use std::time::Duration;

use moor_core::seam::forge::{ForgeError, ForgeRepo, GitAuthor, RepoAt, RepoFiles};
use moor_core::{Secret, Sha};
use serde::Deserialize;
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
use ureq::Agent;
use zeroize::Zeroizing;

/// The public GitHub GraphQL endpoint.
pub(super) const API: &str = "https://api.github.com/graphql";

/// Largest config file accepted, in bytes.
const FILE_CAP: u64 = 64 * 1024;

/// Largest reply body read; GitHub returns file text inline, so this bounds a huge file.
const REPLY_CAP: u64 = 8 * 1024 * 1024;

/// How long the whole request may take.
pub(super) const TIMEOUT: Duration = Duration::from_secs(30);

/// GraphQL error types an error may name; any other is `UNKNOWN`, so no server text gets through.
const ERROR_TYPES: [&str; 7] = [
    "NOT_FOUND",
    "FORBIDDEN",
    "RATE_LIMITED",
    "INSUFFICIENT_SCOPES",
    "SERVICE_UNAVAILABLE",
    "INTERNAL",
    "MAX_NODE_LIMIT_EXCEEDED",
];

/// Most distinct error types an error names.
const ERROR_TYPES_SHOWN: usize = 3;

/// Longest profile name used as the commit author name, in bytes.
const NAME_CAP: usize = 255;

/// Asks for the viewer, the repo's permission, the Box branch tip and the files at one commit.
const QUERY: &str = r#"query($owner: String!, $name: String!, $branch: String!, $base: String!, $hasBase: Boolean!) {
  viewer { login databaseId name }
  repository(owner: $owner, name: $name) {
    viewerPermission
    branch: ref(qualifiedName: $branch) { target { __typename oid } }
    defaultBranchRef @skip(if: $hasBase) { target { ...at } }
    object(expression: $base) @include(if: $hasBase) { ...at }
  }
}
fragment at on GitObject { ...commit ... on Tag { target { ...commit } } }
fragment commit on GitObject {
  __typename
  oid
  ... on Commit {
    moorYaml: file(path: ".moor.yaml") { ...entry }
    mcpJson: file(path: ".mcp.json") { ...entry }
    claudeSettings: file(path: ".claude/settings.json") { ...entry }
  }
}
fragment entry on TreeEntry { object { __typename ... on Blob { byteSize isBinary text } } }"#;

/// Reads `repo` from the GraphQL endpoint `api` as the account of `token`.
pub(super) fn read(
    api: &str,
    timeout: Duration,
    repo: &ForgeRepo,
    base: Option<&str>,
    branch: &str,
    token: &Secret,
) -> Result<RepoAt, ForgeError> {
    let auth = bearer(token)?;
    let body = serde_json::json!({
        "query": QUERY,
        "variables": {
            "owner": repo.owner,
            "name": repo.name,
            "branch": format!("refs/heads/{branch}"),
            "base": base.unwrap_or_default(),
            "hasBase": base.is_some(),
        },
    });
    let (status, reply) = post(api, timeout, &auth, &body.to_string())?;
    match status {
        200 => {
            let reply = serde_json::from_slice(&reply).map_err(|_| unexpected_shape())?;
            parse(reply, base)
        }
        401 => Err(ForgeError::Rejected),
        s => Err(ForgeError::Unavailable(format!("GitHub answered HTTP {s}"))),
    }
}

/// The `Authorization` header value; a token that is not printable ASCII is refused unsent.
fn bearer(token: &Secret) -> Result<Zeroizing<String>, ForgeError> {
    let raw = token.expose();
    if raw.is_empty() || !raw.iter().all(u8::is_ascii_graphic) {
        return Err(ForgeError::Rejected);
    }
    let mut auth = Zeroizing::new(String::with_capacity(raw.len() + 7));
    auth.push_str("Bearer ");
    auth.extend(raw.iter().map(|&b| char::from(b)));
    Ok(auth)
}

/// Posts `body` to `api` directly, without redirects or an env proxy, and returns the status and
/// body.
fn post(
    api: &str,
    timeout: Duration,
    auth: &str,
    body: &str,
) -> Result<(u16, Vec<u8>), ForgeError> {
    let tls = TlsConfig::builder()
        .provider(TlsProvider::Rustls)
        .root_certs(RootCerts::PlatformVerifier)
        .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .build();
    let config = Agent::config_builder()
        .tls_config(tls)
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .max_redirects(0)
        .proxy(None)
        .build();
    let unavailable = |e: ureq::Error| ForgeError::Unavailable(format!("cannot reach GitHub: {e}"));
    let mut response = Agent::new_with_config(config)
        .post(api)
        .header("Authorization", auth)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("User-Agent", "moor")
        .send(body)
        .map_err(unavailable)?;
    let status = response.status().as_u16();
    let bytes = response
        .body_mut()
        .with_config()
        // ureq refuses a body that reaches its limit, not only one that passes it.
        .limit(REPLY_CAP + 1)
        .read_to_vec()
        .map_err(unavailable)?;
    Ok((status, bytes))
}

/// A GraphQL reply.
#[derive(Deserialize)]
struct Reply {
    /// The answer, absent when the whole request failed.
    data: Option<Data>,
    /// Errors, each with a GitHub error type such as `NOT_FOUND`.
    #[serde(default)]
    errors: Vec<GqlError>,
}

/// One GraphQL error; only its type is kept, so no server text reaches an error.
#[derive(Deserialize)]
struct GqlError {
    /// GitHub's error type.
    #[serde(rename = "type")]
    kind: Option<String>,
}

/// The query's top-level fields.
#[derive(Deserialize)]
struct Data {
    /// The token's account.
    viewer: Option<Viewer>,
    /// The repo, absent when it is not visible.
    repository: Option<Repository>,
}

/// The token's account.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Viewer {
    /// Login name.
    login: String,
    /// Numeric account id, used in the noreply address.
    database_id: u64,
    /// Profile name, when set.
    name: Option<String>,
}

/// The repo fields the query asks for.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Repository {
    /// `ADMIN`, `MAINTAIN`, `WRITE`, `TRIAGE` or `READ`.
    viewer_permission: Option<String>,
    /// The Box branch, absent when it does not exist.
    branch: Option<Ref>,
    /// The default branch, asked for when no base is given.
    default_branch_ref: Option<Ref>,
    /// The object the base resolves to, asked for when a base is given.
    object: Option<Object>,
}

/// A git ref.
#[derive(Deserialize)]
struct Ref {
    /// The object the ref points at.
    target: Option<Object>,
}

/// A git object: a commit with its files, or an annotated tag with its target.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Object {
    /// `Commit`, `Tag`, `Tree` or `Blob`.
    #[serde(rename = "__typename")]
    kind: String,
    /// The object id.
    oid: Option<String>,
    /// A tag's target.
    target: Option<Box<Self>>,
    /// `.moor.yaml` at a commit.
    moor_yaml: Option<Entry>,
    /// `.mcp.json` at a commit.
    mcp_json: Option<Entry>,
    /// `.claude/settings.json` at a commit.
    claude_settings: Option<Entry>,
}

/// A path in a commit's tree.
#[derive(Deserialize)]
struct Entry {
    /// What the path holds.
    object: Option<Blob>,
}

/// A path's object; the blob fields are absent when the path is a tree.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Blob {
    /// Size in bytes.
    byte_size: Option<u64>,
    /// Whether GitHub treats the blob as binary.
    is_binary: Option<bool>,
    /// The text, absent for binary blobs.
    text: Option<String>,
}

/// Turns a 200 reply into a [`RepoAt`].
fn parse(reply: Reply, base: Option<&str>) -> Result<RepoAt, ForgeError> {
    let not_found = |e: &GqlError| e.kind.as_deref() == Some("NOT_FOUND");
    let data = reply.data.unwrap_or(Data {
        viewer: None,
        repository: None,
    });
    let Some(repo) = data.repository else {
        return if reply.errors.iter().all(not_found) {
            Err(ForgeError::NotVisible)
        } else {
            Err(unavailable_errors(&reply.errors))
        };
    };
    if !reply.errors.iter().all(not_found) {
        return Err(unavailable_errors(&reply.errors));
    }
    let viewer = data
        .viewer
        .ok_or_else(|| ForgeError::Unavailable("GitHub returned no viewer".to_owned()))?;
    let unknown = || ForgeError::UnknownBase(base.unwrap_or("the default branch").to_owned());
    let at = match base {
        Some(_) => repo.object,
        None => repo.default_branch_ref.and_then(|r| r.target),
    };
    let commit = peel(at.ok_or_else(unknown)?).ok_or_else(unknown)?;
    Ok(RepoAt {
        base: full_sha(commit.oid.as_deref())?,
        branch_tip: repo
            .branch
            .and_then(|r| r.target)
            .map(|t| full_sha(t.oid.as_deref()))
            .transpose()?,
        files: RepoFiles {
            moor_yaml: text(commit.moor_yaml, ".moor.yaml")?,
            mcp_json: text(commit.mcp_json, ".mcp.json")?,
            claude_settings: text(commit.claude_settings, ".claude/settings.json")?,
        },
        author: author(viewer)?,
        can_push: matches!(
            repo.viewer_permission.as_deref(),
            Some("ADMIN" | "MAINTAIN" | "WRITE")
        ),
    })
}

/// Names the first few distinct GraphQL error types, each from [`ERROR_TYPES`] or `UNKNOWN`.
fn unavailable_errors(errors: &[GqlError]) -> ForgeError {
    let mut kinds: Vec<&str> = Vec::new();
    for e in errors {
        let kind = ERROR_TYPES
            .into_iter()
            .find(|t| e.kind.as_deref() == Some(*t))
            .unwrap_or("UNKNOWN");
        if !kinds.contains(&kind) {
            kinds.push(kind);
        }
        if kinds.len() == ERROR_TYPES_SHOWN {
            break;
        }
    }
    ForgeError::Unavailable(format!("GitHub GraphQL errors: {}", kinds.join(", ")))
}

/// The error for a reply that does not fit the query's shape.
fn unexpected_shape() -> ForgeError {
    ForgeError::Unavailable("GitHub sent a reply of an unexpected shape".to_owned())
}

/// The commit itself, or the commit an annotated tag points at.
fn peel(object: Object) -> Option<Object> {
    let object = match object.kind.as_str() {
        "Tag" => *object.target?,
        _ => object,
    };
    (object.kind == "Commit").then_some(object)
}

/// Parses a full 40-character object id.
fn full_sha(oid: Option<&str>) -> Result<Sha, ForgeError> {
    oid.filter(|o| o.len() == 40)
        .and_then(|o| Sha::parse(o).ok())
        .ok_or_else(|| ForgeError::Unavailable("GitHub returned a malformed object id".to_owned()))
}

/// A config file's text: `None` when absent, unreadable when binary, oversized or not a file.
fn text(entry: Option<Entry>, path: &'static str) -> Result<Option<String>, ForgeError> {
    let Some(entry) = entry else {
        return Ok(None);
    };
    match entry.object {
        Some(Blob {
            byte_size: Some(size),
            is_binary: Some(false),
            text: Some(text),
        }) if size <= FILE_CAP => Ok(Some(text)),
        _ => Err(ForgeError::Unreadable(path)),
    }
}

/// The account's profile name, else its login, with its GitHub noreply address; a login not
/// shaped like GitHub's is refused.
fn author(viewer: Viewer) -> Result<GitAuthor, ForgeError> {
    let login = &viewer.login;
    if !(1..=39).contains(&login.len())
        || !login
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err(unexpected_shape());
    }
    let email = format!("{}+{login}@users.noreply.github.com", viewer.database_id);
    let name = viewer
        .name
        .filter(|n| !n.trim().is_empty() && n.len() <= NAME_CAP && !n.chars().any(char::is_control))
        .unwrap_or(viewer.login);
    Ok(GitAuthor { name, email })
}

#[cfg(test)]
mod tests;
