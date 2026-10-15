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
    /// Forward the request without its credential, with these exact body
    /// bytes. The request is outside the Task repo or could not be judged.
    Anonymous(Vec<u8>),
    /// Refuse with this status and message; the reason is recorded.
    Refuse(u16, String),
    /// Close without a reply.
    Abort,
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
        return Gate::Anonymous(body.raw);
    };
    match judge(&body.decoded, repo) {
        Verdict::Allow => Gate::Forward(body.raw),
        Verdict::Deny(_) => Gate::Anonymous(body.raw),
        Verdict::NeedsNodeCheck(ids) => match nodes_in_repo(shared, head, repo, &ids) {
            Some(true) => Gate::Forward(body.raw),
            Some(false) | None => Gate::Anonymous(body.raw),
        },
    }
}

/// `head` without any `Authorization` header, in both `raw` and `headers`.
pub(crate) fn without_credential(head: &Head) -> Head {
    let mut raw = Vec::with_capacity(head.raw.len());
    for (i, line) in head.raw.split_inclusive(|&b| b == b'\n').enumerate() {
        let name = line.split(|&b| b == b':').next().unwrap_or_default();
        if i > 0 && name.eq_ignore_ascii_case(b"authorization") {
            continue;
        }
        raw.extend_from_slice(line);
    }
    Head {
        raw,
        headers: head
            .headers
            .iter()
            .filter(|(n, _)| !n.eq_ignore_ascii_case("authorization"))
            .cloned()
            .collect(),
        ..head.clone()
    }
}

/// Whether every id belongs to `repo`, asking upstream about uncached ones.
fn nodes_in_repo(shared: &Shared, head: &Head, repo: &RepoRef, ids: &[NodeId]) -> Option<bool> {
    let uncached: Vec<NodeId> = ids
        .iter()
        .filter(|id| shared.cache.get(id).is_none())
        .cloned()
        .collect();
    if !uncached.is_empty() {
        let auth = head.header("authorization")?;
        let verdicts = lookup(shared, repo, auth, &uncached)?;
        for (id, belongs) in uncached.iter().zip(verdicts) {
            shared.cache.put(id, belongs);
        }
    }
    Some(ids.iter().all(|id| shared.cache.get(id) == Some(true)))
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
