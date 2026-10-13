//! GitHub proxy route rows and origin URL parsing.

use moor_core::proxy_gate::RouteRow;
use moor_core::seam::forge::ForgeRepo;

/// The only host whose origins are accepted; GitHub Enterprise hosts are not.
const HOST: &str = "github.com";

/// Returns the git transport row and the pull-request API row for GitHub.
pub(super) fn routes() -> Vec<RouteRow> {
    vec![
        RouteRow::forge_git("/github"),
        RouteRow::forge_api("/github-api"),
    ]
}

/// Parses a git origin URL into its owner and repo name.
///
/// Accepts `https://` and `ssh://` URLs and the scp form `git@github.com:owner/name`, with an
/// optional `.git` suffix and one trailing slash. Any other host, scheme, port, or path shape is
/// refused.
pub(super) fn parse_origin(origin: &str) -> Option<ForgeRepo> {
    let (authority, path) = if let Some(rest) = origin
        .strip_prefix("https://")
        .or_else(|| origin.strip_prefix("ssh://"))
    {
        rest.split_once('/')?
    } else if origin.contains("://") {
        return None;
    } else {
        origin.split_once(':')?
    };
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    if !host.eq_ignore_ascii_case(HOST) {
        return None;
    }
    repo_from_path(path)
}

/// Splits `owner/name[.git][/]` into a repo; exactly two valid segments are required.
fn repo_from_path(path: &str) -> Option<ForgeRepo> {
    let path = path.strip_suffix('/').unwrap_or(path);
    let path = path.strip_suffix(".git").unwrap_or(path);
    let (owner, name) = path.split_once('/')?;
    (valid_segment(owner) && valid_segment(name)).then(|| ForgeRepo::new(owner, name))
}

/// Reports whether `s` is a usable owner or repo name: non-empty, no whitespace, no slash, and
/// not a relative path component.
fn valid_segment(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && !s.contains('/')
        && !s.chars().any(char::is_whitespace)
}

#[cfg(test)]
mod tests;
