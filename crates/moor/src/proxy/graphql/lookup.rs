//! Node-id lookup that resolves GraphQL node ids to their repository.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Mutex, MutexGuard, PoisonError};

use moor_core::graphql_scope::{NodeId, RepoRef};
use serde_json::{json, Value};

/// Query asking GitHub which repository each node id belongs to.
const QUERY: &str = "query($ids:[ID!]!){nodes(ids:$ids){__typename \
    ... on Repository{nameWithOwner} \
    ... on PullRequest{repository{nameWithOwner}} \
    ... on Issue{repository{nameWithOwner}}}}";

/// Builds the full HTTP/1.1 lookup request, carrying `authorization` verbatim.
pub(crate) fn lookup_request(host: &str, authorization: &str, ids: &[NodeId]) -> Vec<u8> {
    let body = lookup_body(ids);
    let mut out = format!(
        "POST /graphql HTTP/1.1\r\nHost: {host}\r\nAuthorization: {authorization}\r\n\
         User-Agent: moor\r\nContent-Type: application/json\r\nAccept: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    out.extend_from_slice(&body);
    out
}

/// Builds the JSON lookup body, passing ids as GraphQL variables.
pub(crate) fn lookup_body(ids: &[NodeId]) -> Vec<u8> {
    let ids: Vec<&str> = ids.iter().map(NodeId::as_str).collect();
    serde_json::to_vec(&json!({ "query": QUERY, "variables": { "ids": ids } })).unwrap_or_default()
}

/// Reads per-id membership of `repo` from a lookup response, in `ids` order.
///
/// Returns `None` when the body is not a usable answer.
pub(crate) fn resolve(body: &[u8], ids: &[NodeId], repo: &RepoRef) -> Option<Vec<bool>> {
    let root: Value = serde_json::from_slice(body).ok()?;
    if root.get("errors").is_some() {
        return None;
    }
    let nodes = root.get("data")?.get("nodes")?.as_array()?;
    if nodes.len() != ids.len() {
        return None;
    }
    Some(nodes.iter().map(|node| in_repo(node, repo)).collect())
}

/// Whether one looked-up node is, or belongs to, `repo`.
fn in_repo(node: &Value, repo: &RepoRef) -> bool {
    let name = match node.get("__typename").and_then(Value::as_str) {
        Some("Repository") => node.get("nameWithOwner"),
        Some("PullRequest" | "Issue") => {
            node.get("repository").and_then(|r| r.get("nameWithOwner"))
        }
        _ => None,
    };
    name.and_then(Value::as_str).is_some_and(|full| {
        full.split_once('/').is_some_and(|(owner, name)| {
            owner.eq_ignore_ascii_case(&repo.owner) && name.eq_ignore_ascii_case(&repo.name)
        })
    })
}

/// In-memory per-Box cache of node-id verdicts.
#[derive(Default)]
pub(crate) struct NodeCache(Mutex<HashMap<String, bool>>);

impl NodeCache {
    /// Creates an empty cache.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Returns the cached verdict for `id`, if any.
    pub(crate) fn get(&self, id: &NodeId) -> Option<bool> {
        self.lock().get(id.as_str()).copied()
    }

    /// Records whether `id` belongs to the Box repository.
    pub(crate) fn put(&self, id: &NodeId, belongs: bool) {
        self.lock().insert(id.as_str().to_owned(), belongs);
    }

    /// Locks the map, recovering from poisoning.
    fn lock(&self) -> MutexGuard<'_, HashMap<String, bool>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl fmt::Debug for NodeCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NodeCache")
            .field("entries", &self.lock().len())
            .finish()
    }
}

#[cfg(test)]
mod tests;
