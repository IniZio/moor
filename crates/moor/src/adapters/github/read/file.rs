//! Reading one file of a commit.

use std::time::Duration;

use moor_core::seam::forge::{ForgeError, ForgeRepo};
use moor_core::{Secret, Sha};

use super::{bearer, commit_at, full_sha, post, text, unexpected_shape, visible};

/// Asks for the commit a ref names, or the default branch head, and one file there.
const FILE_QUERY: &str = r"query($owner: String!, $name: String!, $at: String!, $hasAt: Boolean!, $path: String!) {
  repository(owner: $owner, name: $name) {
    defaultBranchRef @skip(if: $hasAt) { target { ...at } }
    object(expression: $at) @include(if: $hasAt) { ...at }
  }
}
fragment at on GitObject { ...commit ... on Tag { target { ...commit } } }
fragment commit on GitObject {
  __typename
  oid
  ... on Commit { file(path: $path) { object { __typename ... on Blob { byteSize isBinary text } } } }
}";

/// Reads `path` in `repo` at `at` (the default branch when `None`) from the GraphQL endpoint
/// `api` as the account of `token`, with the commit `at` resolved to.
pub(in crate::adapters::github) fn read_file_at(
    api: &str,
    timeout: Duration,
    repo: &ForgeRepo,
    at: Option<&str>,
    path: &'static str,
    token: &Secret,
) -> Result<(Sha, Option<String>), ForgeError> {
    let auth = bearer(token)?;
    let body = serde_json::json!({
        "query": FILE_QUERY,
        "variables": {
            "owner": repo.owner,
            "name": repo.name,
            "at": at.unwrap_or_default(),
            "hasAt": at.is_some(),
            "path": path,
        },
    });
    let (status, reply) = post(api, timeout, &auth, &body.to_string())?;
    match status {
        200 => {
            let reply = serde_json::from_slice(&reply).map_err(|_| unexpected_shape())?;
            let (_, mut found) = visible(reply)?;
            let commit = commit_at(&mut found, at)?;
            Ok((full_sha(commit.oid.as_deref())?, text(commit.file, path)?))
        }
        401 => Err(ForgeError::Rejected),
        s => Err(ForgeError::Unavailable(format!("GitHub answered HTTP {s}"))),
    }
}
