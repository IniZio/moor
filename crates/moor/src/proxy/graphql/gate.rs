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
    /// Forward the request with the placeholder in place of its credential,
    /// with these exact body bytes. The request is outside the Box repos or
    /// could not be judged.
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
    let in_scope = |repo: &RepoRef| match judge(&body.decoded, repo) {
        Verdict::Allow => true,
        Verdict::Deny(_) => false,
        Verdict::NeedsNodeCheck(ids) => nodes_listed(shared, head, &ids) == Some(true),
    };
    if shared.repos.iter().any(in_scope) {
        Gate::Forward(body.raw)
    } else {
        Gate::Anonymous(body.raw)
    }
}

/// `head` without `Authorization` or any of the `injected` header names, in
/// both `raw` and `headers`. `placeholder`, if any, is then the only
/// `Authorization`.
pub(super) fn without_credential(
    head: &Head,
    injected: &[String],
    placeholder: Option<&str>,
) -> Head {
    let strip = |name: &[u8]| {
        name.eq_ignore_ascii_case(b"authorization")
            || injected
                .iter()
                .any(|h| name.eq_ignore_ascii_case(h.as_bytes()))
    };
    let stand_in = placeholder.map(|p| ("authorization".to_owned(), format!("bearer {p}")));
    let mut raw = Vec::with_capacity(head.raw.len());
    for (i, line) in head.raw.split_inclusive(|&b| b == b'\n').enumerate() {
        let name = line.split(|&b| b == b':').next().unwrap_or_default();
        if i > 0 && strip(name) {
            continue;
        }
        raw.extend_from_slice(line);
        if let (0, Some((name, value))) = (i, &stand_in) {
            raw.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
        }
    }
    Head {
        raw,
        headers: stand_in
            .iter()
            .chain(head.headers.iter().filter(|(n, _)| !strip(n.as_bytes())))
            .cloned()
            .collect(),
        ..head.clone()
    }
}

/// Whether every id belongs to a listed repo, asking upstream about uncached ones.
fn nodes_listed(shared: &Shared, head: &Head, ids: &[NodeId]) -> Option<bool> {
    let uncached: Vec<NodeId> = ids
        .iter()
        .filter(|id| shared.cache.get(id).is_none())
        .cloned()
        .collect();
    if !uncached.is_empty() {
        let auth = head.header("authorization")?;
        let verdicts = lookup(shared, auth, &uncached)?;
        for (id, belongs) in uncached.iter().zip(verdicts) {
            shared.cache.put(id, belongs);
        }
    }
    Some(ids.iter().all(|id| shared.cache.get(id) == Some(true)))
}

/// Asks upstream, on a new connection, which of `ids` are in a listed repo.
fn lookup(shared: &Shared, auth: &str, ids: &[NodeId]) -> Option<Vec<bool>> {
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
    resolve(&body.decoded, ids, &shared.repos)
}
