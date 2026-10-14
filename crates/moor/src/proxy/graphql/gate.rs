//! Decides whether a request to `/graphql` may go upstream.

use std::io::{BufRead, BufReader, Write};

use moor_core::graphql_scope::{judge, NodeId, RepoRef, Verdict};

use super::http::{
    read_body, read_response_head, request_framing, response_framing, Head, HttpError,
};
use super::lookup::{lookup_request, resolve};
use super::Shared;

/// Largest GraphQL request body judged, in bytes.
const MAX_BODY: usize = 64 * 1024;
/// Largest node-lookup response read, in bytes.
const MAX_LOOKUP: usize = 1024 * 1024;

/// What to do with a GraphQL request.
pub(super) enum Gate {
    /// Forward the request head and these exact body bytes.
    Forward(Vec<u8>),
    /// Refuse with this status and message; the reason is recorded.
    Refuse(u16, String),
    /// Close without a reply.
    Abort,
}

/// A refusal for `reason`.
pub(super) fn deny(reason: &str) -> Gate {
    Gate::Refuse(403, format!("moor: {reason}"))
}

/// Reads the body of a request to `/graphql` and judges it.
pub(super) fn gate(shared: &Shared, head: &Head, r: &mut impl BufRead) -> Gate {
    let framing = match request_framing(head) {
        Ok(f) => f,
        Err(HttpError::Malformed | HttpError::TooLarge) => {
            return Gate::Refuse(400, "moor: malformed request".to_owned())
        }
        Err(_) => return Gate::Abort,
    };
    let body = match read_body(r, &framing, MAX_BODY) {
        Ok(b) => b,
        Err(HttpError::TooLarge) => {
            return Gate::Refuse(413, "moor: graphql body too large".to_owned())
        }
        Err(HttpError::Malformed) => {
            return Gate::Refuse(400, "moor: malformed request".to_owned())
        }
        Err(_) => return Gate::Abort,
    };
    let Some(repo) = shared.repo.as_ref() else {
        return deny("no Task repo");
    };
    match judge(&body.decoded, repo) {
        Verdict::Allow => Gate::Forward(body.raw),
        Verdict::Deny(reason) => deny(reason.as_str()),
        Verdict::NeedsNodeCheck(ids) => match nodes_in_repo(shared, head, repo, &ids) {
            Ok(true) => Gate::Forward(body.raw),
            Ok(false) => deny("node id outside the Task repo"),
            Err(why) => deny(why),
        },
    }
}

/// Whether every id belongs to `repo`, asking upstream about uncached ones.
fn nodes_in_repo(
    shared: &Shared,
    head: &Head,
    repo: &RepoRef,
    ids: &[NodeId],
) -> Result<bool, &'static str> {
    let uncached: Vec<NodeId> = ids
        .iter()
        .filter(|id| shared.cache.get(id).is_none())
        .cloned()
        .collect();
    if !uncached.is_empty() {
        let auth = head
            .header("authorization")
            .ok_or("node lookup needs credentials")?;
        let verdicts = lookup(shared, repo, auth, &uncached).ok_or("node lookup failed")?;
        for (id, belongs) in uncached.iter().zip(verdicts) {
            shared.cache.put(id, belongs);
        }
    }
    Ok(ids.iter().all(|id| shared.cache.get(id) == Some(true)))
}

/// Asks upstream, on a new connection, which of `ids` are in `repo`.
fn lookup(shared: &Shared, repo: &RepoRef, auth: &str, ids: &[NodeId]) -> Option<Vec<bool>> {
    let mut conn = BufReader::new(shared.upstream.connect().ok()?);
    let request = lookup_request(shared.upstream.host(), auth, ids);
    conn.get_mut().write_all(&request).ok()?;
    conn.get_mut().flush().ok()?;
    let head = read_response_head(&mut conn).ok()?;
    if head.status != 200 {
        return None;
    }
    let framing = response_framing(&head, "POST").ok()?;
    let body = read_body(&mut conn, &framing, MAX_LOOKUP).ok()?;
    resolve(&body.decoded, ids, repo)
}
