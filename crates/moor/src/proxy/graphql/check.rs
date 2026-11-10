//! The response check of a request that reads linked issues: their title and state reach
//! the Box only when their repo is a Box repo, see [`Linked::blank`].

use std::io::BufRead;

use moor_core::graphql_scope::{Linked, RepoRef};

use super::http::{read_body, Framing, Head, ResponseHead};

/// Largest response body checked, in bytes.
const MAX_CHECKED: usize = 32 * 1024 * 1024;

/// How a response is relayed.
#[derive(Clone, Copy)]
pub(super) enum Relay<'a> {
    /// Byte for byte.
    Plain,
    /// Byte for byte, for a request sent without its credential.
    Anonymous,
    /// After blanking the linked issues outside the Box repos.
    Checked(&'a Linked),
}

/// Response headers dropped from a checked response, which is framed by its length.
const REFRAMED: &[&str] = &["content-length", "transfer-encoding", "trailer"];

/// `head` asking upstream for an unencoded response, so the check can read it.
pub(super) fn unencoded(head: &Head) -> Head {
    let drop = |name: &[u8]| name.eq_ignore_ascii_case(b"accept-encoding");
    let mut raw = Vec::with_capacity(head.raw.len());
    for (i, line) in head.raw.split_inclusive(|&b| b == b'\n').enumerate() {
        let name = line.split(|&b| b == b':').next().unwrap_or_default();
        if i > 0 && drop(name) {
            continue;
        }
        raw.extend_from_slice(line);
        if i == 0 {
            raw.extend_from_slice(b"accept-encoding: identity\r\n");
        }
    }
    let identity = ("accept-encoding".to_owned(), "identity".to_owned());
    Head {
        raw,
        headers: std::iter::once(identity)
            .chain(
                head.headers
                    .iter()
                    .filter(|(n, _)| !drop(n.as_bytes()))
                    .cloned(),
            )
            .collect(),
        ..head.clone()
    }
}

/// Reads the response body from `r`, blanks the linked issues outside `repos`, and returns
/// the whole response to relay, framed by its length. Fails, with the refusal message,
/// when the body is encoded, too large, unframed or not JSON.
pub(super) fn checked(
    resp: &ResponseHead,
    framing: &Framing,
    r: &mut impl BufRead,
    linked: &Linked,
    repos: &[RepoRef],
) -> Result<Vec<u8>, String> {
    let fail = |why: &str| format!("moor: response could not be checked: {why}");
    if *framing == Framing::Empty {
        return Ok(resp.raw.clone());
    }
    let encoded = resp
        .headers
        .iter()
        .filter(|(n, _)| n.eq_ignore_ascii_case("content-encoding"))
        .any(|(_, v)| !v.trim().eq_ignore_ascii_case("identity"));
    if encoded {
        return Err(fail("encoded response"));
    }
    let body = read_body(r, framing, MAX_CHECKED).map_err(|_| fail("unreadable response body"))?;
    let body = if body.decoded.is_empty() {
        Vec::new()
    } else {
        let mut json =
            serde_json::from_slice(&body.decoded).map_err(|_| fail("response is not json"))?;
        linked.blank(&mut json, repos);
        serde_json::to_vec(&json).map_err(|_| fail("response is not json"))?
    };
    let mut out = Vec::with_capacity(resp.raw.len() + body.len());
    for (i, line) in resp.raw.split_inclusive(|&b| b == b'\n').enumerate() {
        let name = line.split(|&b| b == b':').next().unwrap_or_default();
        if line == b"\r\n"
            || (i > 0
                && REFRAMED
                    .iter()
                    .any(|h| name.eq_ignore_ascii_case(h.as_bytes())))
        {
            continue;
        }
        out.extend_from_slice(line);
    }
    out.extend_from_slice(format!("content-length: {}\r\n\r\n", body.len()).as_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}
