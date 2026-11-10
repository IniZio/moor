//! The response check of a request that reads linked issues: their title and state reach
//! the Box only when their repo is a Box repo, see [`Linked::blank`].

use std::io::BufRead;
use std::sync::{Condvar, Mutex, PoisonError};

use moor_core::graphql_scope::{Linked, RepoRef};

use super::http::{read_body, Framing, Head, ResponseHead};

/// Largest response body checked, in bytes: `gh issue view --comments` reads 100 comments
/// of up to 64 KiB and 100 sub-issues.
const MAX_CHECKED: usize = 8 * 1024 * 1024;

/// Most checked requests one filter, so one Box, has in flight at once.
const MAX_IN_FLIGHT: usize = 1;

/// Bounds the memory checked responses take: each is held whole, so a Box gets
/// [`MAX_IN_FLIGHT`] at once and the rest wait their turn.
#[derive(Default)]
pub(super) struct Turns {
    /// Checked requests in flight.
    busy: Mutex<usize>,
    /// Signalled when one ends.
    freed: Condvar,
}

impl Turns {
    /// Waits for a free turn, held until the returned guard drops.
    pub(super) fn take(&self) -> Turn<'_> {
        let busy = self.busy.lock().unwrap_or_else(PoisonError::into_inner);
        let mut busy = self
            .freed
            .wait_while(busy, |n| *n >= MAX_IN_FLIGHT)
            .unwrap_or_else(PoisonError::into_inner);
        *busy += 1;
        drop(busy);
        Turn(self)
    }
}

/// One checked request in flight.
pub(super) struct Turn<'a>(&'a Turns);

impl Drop for Turn<'_> {
    fn drop(&mut self) {
        *self.0.busy.lock().unwrap_or_else(PoisonError::into_inner) -= 1;
        self.0.freed.notify_one();
    }
}

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
/// the head and body to relay, framed by its length. Fails, with the refusal message,
/// when the body is encoded, too large, unframed or not JSON.
pub(super) fn checked(
    resp: &ResponseHead,
    framing: &Framing,
    r: &mut impl BufRead,
    linked: &Linked,
    repos: &[RepoRef],
) -> Result<(Vec<u8>, Vec<u8>), String> {
    let fail = |why: &str| format!("moor: response could not be checked: {why}");
    if *framing == Framing::Empty {
        return Ok((resp.raw.clone(), Vec::new()));
    }
    let encoded = resp
        .headers
        .iter()
        .filter(|(n, _)| n.eq_ignore_ascii_case("content-encoding"))
        .any(|(_, v)| !v.trim().eq_ignore_ascii_case("identity"));
    if encoded {
        return Err(fail("encoded response"));
    }
    let read = read_body(r, framing, MAX_CHECKED, &mut std::io::sink())
        .map_err(|_| fail("unreadable response body"))?;
    let body = if read.is_empty() {
        read
    } else {
        let body = linked
            .blank(&read, repos)
            .map_err(|_| fail("response is not json"))?;
        drop(read);
        body
    };
    let mut out = Vec::with_capacity(resp.raw.len());
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
    Ok((out, body))
}
