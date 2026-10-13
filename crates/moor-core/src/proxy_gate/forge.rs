//! Forge allow-lists: the only git transport and pull-request calls a Box agent may make, and only
//! against the repository of its Task.

use super::{Allow, Inject, Reject, RouteId, RouteRow};

impl RouteRow {
    /// Returns a forge git transport route under `prefix`; a trailing slash is dropped.
    #[must_use]
    pub fn forge_git(prefix: impl Into<String>) -> Self {
        Self::forge(
            RouteId::new("forge.git"),
            &prefix.into(),
            Allow::GitSmartHttp,
        )
    }

    /// Returns a forge pull-request API route under `prefix`; a trailing slash is dropped.
    #[must_use]
    pub fn forge_api(prefix: impl Into<String>) -> Self {
        Self::forge(RouteId::new("forge.api"), &prefix.into(), Allow::PullsApi)
    }

    /// Builds a forge row that injects the row credential and strips no extra headers.
    fn forge(id: RouteId, prefix: &str, allow: Allow) -> Self {
        Self {
            id,
            prefix: prefix.trim_end_matches('/').to_owned(),
            allow,
            strip: Vec::new(),
            inject: Inject::Bearer,
        }
    }
}

/// Query of a git service advertisement, the only query the transport admits.
const SERVICES: [&str; 2] = ["service=git-upload-pack", "service=git-receive-pack"];

/// Returns whether a method is the exact text of one of `allowed`.
fn method_in(method: &str, allowed: &[&str]) -> bool {
    allowed.contains(&method)
}

/// Splits a path below the route prefix into segments.
///
/// The path must start with `/` and have no empty segment, no `..` segment and no `%`, so that
/// percent-encoded traversal and doubled or trailing slashes never reach the forge.
fn segments(rest: &str) -> Result<Vec<&str>, Reject> {
    let path = rest
        .strip_prefix('/')
        .ok_or(Reject::Forbidden("path must start with /"))?;
    if rest.contains('%') {
        return Err(Reject::Forbidden("percent-encoding is not admitted"));
    }
    let segments: Vec<&str> = path.split('/').collect();
    if segments.iter().any(|s| s.is_empty()) {
        return Err(Reject::Forbidden("empty path segment"));
    }
    if segments.contains(&"..") {
        return Err(Reject::Forbidden("path traversal"));
    }
    Ok(segments)
}

/// Returns a repository name without an optional `.git` suffix.
fn bare(name: &str) -> &str {
    name.strip_suffix(".git").unwrap_or(name)
}

/// Returns whether a segment is a plausible owner or repository name.
fn name_ok(segment: &str) -> bool {
    !segment.is_empty()
        && segment != "."
        && segment != ".."
        && segment
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

/// Checks that `owner/name` from the request is the Task repository `owner/name`.
fn same_repo(owner: &str, name: &str, task_repo: Option<&str>) -> Result<(), Reject> {
    let task_repo = task_repo.ok_or(Reject::Forbidden("task has no forge repository"))?;
    let (t_owner, t_name) = task_repo
        .split_once('/')
        .filter(|(o, n)| name_ok(o) && name_ok(n) && !n.contains('/'))
        .map(|(o, n)| (o, bare(n)))
        .filter(|(_, n)| !n.is_empty())
        .ok_or(Reject::Forbidden("task repository is malformed"))?;
    let name = bare(name);
    if !name_ok(owner) || !name_ok(name) || name.is_empty() {
        return Err(Reject::Forbidden("malformed repository"));
    }
    if owner.eq_ignore_ascii_case(t_owner) && name.eq_ignore_ascii_case(t_name) {
        Ok(())
    } else {
        Err(Reject::Forbidden("repository is not the task repository"))
    }
}

/// Admits git smart-HTTP requests for the Task repository.
///
/// `rest` is the path after the route prefix.
pub(super) fn admit_git(
    method: &str,
    rest: &str,
    query: Option<&str>,
    task_repo: Option<&str>,
) -> Result<(), Reject> {
    let segs = segments(rest)?;
    let [owner, name, tail @ ..] = segs.as_slice() else {
        return Err(Reject::Forbidden("not a git endpoint"));
    };
    same_repo(owner, name, task_repo)?;
    match tail {
        ["info", "refs"] => {
            if !method_in(method, &["GET", "HEAD"]) {
                return Err(Reject::BadMethod);
            }
            if query.is_some_and(|q| SERVICES.contains(&q)) {
                Ok(())
            } else {
                Err(Reject::Forbidden("unknown git service"))
            }
        }
        ["git-upload-pack" | "git-receive-pack"] => {
            if method != "POST" {
                Err(Reject::BadMethod)
            } else if query.is_some() {
                Err(Reject::Forbidden("query not admitted"))
            } else {
                Ok(())
            }
        }
        _ => Err(Reject::Forbidden("not a git endpoint")),
    }
}

/// Admits the pull-request API calls for the Task repository.
///
/// `rest` is the path after the route prefix.
pub(super) fn admit_api(
    method: &str,
    rest: &str,
    query: Option<&str>,
    task_repo: Option<&str>,
) -> Result<(), Reject> {
    let segs = segments(rest)?;
    let ["repos", owner, name, tail @ ..] = segs.as_slice() else {
        return Err(Reject::Forbidden("not an admitted API call"));
    };
    same_repo(owner, name, task_repo)?;
    let no_query = || {
        if query.is_some() {
            Err(Reject::Forbidden("query not admitted"))
        } else {
            Ok(())
        }
    };
    match tail {
        [] if method == "GET" => no_query(),
        ["pulls"] if method == "GET" => Ok(()),
        ["pulls"] if method == "POST" => no_query(),
        ["pulls", number]
            if method == "GET"
                && !number.is_empty()
                && number.bytes().all(|b| b.is_ascii_digit()) =>
        {
            no_query()
        }
        [] | ["pulls"] => Err(Reject::BadMethod),
        ["pulls", number] if number.bytes().all(|b| b.is_ascii_digit()) => Err(Reject::BadMethod),
        _ => Err(Reject::Forbidden("not an admitted API call")),
    }
}

#[cfg(test)]
mod tests;
