//! Judges `/repositories/{id}/...`, the numeric-id form GitHub uses in pagination links and
//! some redirects. Only a Box repo's id keeps the credential, and only for a path its
//! `/repos/{owner}/{name}/...` form would allow.

use std::collections::HashMap;
use std::io::{BufReader, Write};
use std::sync::{Mutex, MutexGuard, PoisonError};

use moor_core::graphql_scope::RepoRef;
use serde_json::Value;

use super::http::{read_body, read_response_head, response_framing, Head};
use super::Shared;

/// The path prefix of the numeric-id form.
pub(super) const PREFIX: &str = "/repositories";
/// Largest repo metadata response read, in bytes.
const MAX_REPO: usize = 1024 * 1024;

/// What to do with a request on the numeric-id form.
pub(super) enum IdGate {
    /// The request carries no credential: send it as it came.
    Unchanged,
    /// Send it with its credential.
    Forward,
    /// Send it without its credential, for the reason given.
    Anonymous(&'static str),
}

/// Decides whether the credential nono added to a `/repositories/...` request may stay.
pub(super) fn gate_id(shared: &Shared, head: &Head, canonical: &str) -> IdGate {
    let Some(auth) = head
        .header("authorization")
        .filter(|a| !is_placeholder(shared, a))
    else {
        return IdGate::Unchanged;
    };
    let path = head.target.split(['?', '#']).next().unwrap_or_default();
    let Some((id, tail)) = split_id(path).filter(|_| path == canonical) else {
        return IdGate::Anonymous("repository id path is not canonical");
    };
    let repo = match shared.repo_ids.box_repo(shared, auth, id) {
        Found::Repo(repo) => repo,
        Found::Other => return IdGate::Anonymous("repository id is not a Box repo"),
        Found::Unknown => return IdGate::Anonymous("repository id lookup failed"),
    };
    let repos_form = format!("/repos/{}/{}{tail}", repo.owner, repo.name);
    if shared.rest.is_allowed(&head.method, &repos_form) {
        IdGate::Forward
    } else {
        IdGate::Anonymous("no allow entry matches the request for the Box repo")
    }
}

/// Which repo a numeric id names.
enum Found<'a> {
    /// This Box repo.
    Repo(&'a RepoRef),
    /// No Box repo: every Box repo's id is known and none matches.
    Other,
    /// No Box repo whose id is known; a lookup failed.
    Unknown,
}

/// Whether `auth` is the Box's own placeholder, which nono leaves when it adds nothing.
fn is_placeholder(shared: &Shared, auth: &str) -> bool {
    shared
        .placeholder
        .as_deref()
        .is_some_and(|p| auth.split_ascii_whitespace().last() == Some(p))
}

/// The id and the rest of `/repositories/{id}<tail>`; the id is plain decimal with no
/// leading zero.
fn split_id(path: &str) -> Option<(u64, &str)> {
    let rest = path.strip_prefix(PREFIX)?.strip_prefix('/')?;
    let (digits, tail) = rest.find('/').map_or((rest, ""), |i| rest.split_at(i));
    let plain = !digits.is_empty()
        && digits.bytes().all(|b| b.is_ascii_digit())
        && !(digits.len() > 1 && digits.starts_with('0'));
    plain
        .then(|| digits.parse().ok())
        .flatten()
        .map(|id| (id, tail))
}

/// Numeric ids of the Box repos, by index into the Box repos, learned on first use.
#[derive(Debug, Default)]
pub(crate) struct RepoIds(Mutex<HashMap<usize, u64>>);

impl RepoIds {
    /// The Box repo whose id is `id`, asking upstream with `auth` for ids not yet learned.
    /// A failed lookup is not remembered, so the next request asks again.
    fn box_repo<'a>(&self, shared: &'a Shared, auth: &str, id: u64) -> Found<'a> {
        let mut found = Found::Other;
        for (i, repo) in shared.repos.iter().enumerate() {
            let known = self.lock().get(&i).copied();
            let learned = known.or_else(|| {
                let fetched = fetch_id(shared, auth, repo)?;
                self.lock().insert(i, fetched);
                Some(fetched)
            });
            match learned {
                Some(learned) if learned == id => return Found::Repo(repo),
                Some(_) => {}
                None => found = Found::Unknown,
            }
        }
        found
    }

    /// Locks the map, recovering from poisoning.
    fn lock(&self) -> MutexGuard<'_, HashMap<usize, u64>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Asks upstream, on a new connection, for the numeric id of `repo`.
fn fetch_id(shared: &Shared, auth: &str, repo: &RepoRef) -> Option<u64> {
    let mut conn = BufReader::new(shared.upstream.connect().ok()?);
    let request = format!(
        "GET /repos/{}/{} HTTP/1.1\r\nHost: {}\r\nAuthorization: {auth}\r\nUser-Agent: moor\r\n\
         Accept: application/vnd.github+json\r\nConnection: close\r\n\r\n",
        repo.owner,
        repo.name,
        shared.upstream.host()
    );
    conn.get_mut().write_all(request.as_bytes()).ok()?;
    conn.get_mut().flush().ok()?;
    let head = read_response_head(&mut conn).ok()?;
    if head.status != 200 {
        return None;
    }
    let framing = response_framing(&head, "GET").ok()?;
    let body = read_body(&mut conn, &framing, MAX_REPO, &mut std::io::sink()).ok()?;
    let root: Value = serde_json::from_slice(&body).ok()?;
    root.get("id")?.as_u64()
}

#[cfg(test)]
mod tests;
