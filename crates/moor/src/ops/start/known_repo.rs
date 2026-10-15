//! A repo the controller knows: it is cloned and kept on the controller host, not read from the
//! invocation directory.

use std::path::PathBuf;

use moor_core::config::find_repo;
use moor_core::{ErrorKind, StartReq};

use crate::cx::Cx;
use crate::error::CommandError;

/// The repo root in the controller's cache, and `req` with the base filled from the repo's default
/// branch when it has one and no base was given.
///
/// The clone uses the host's own git credentials.
///
/// # Errors
/// Usage with `--worktree`; Refused for an unknown repo; Failed when the clone or fetch fails.
pub(super) fn prepare(
    cx: &Cx,
    req: &StartReq,
    key: &str,
) -> Result<(PathBuf, StartReq), CommandError> {
    if req.worktree.is_some() {
        return Err(CommandError::new(
            ErrorKind::Usage,
            "--worktree names a path on the client and cannot be combined with --repo",
        ));
    }
    let repos = cx.known_repos();
    let Some((name, entry)) = find_repo(repos, key) else {
        let known: Vec<&str> = repos.keys().map(String::as_str).collect();
        let tail = if known.is_empty() {
            "no repos are configured".to_owned()
        } else {
            format!("known repos: {}", known.join(", "))
        };
        return Err(CommandError::new(
            ErrorKind::Refused,
            format!("repo {key} is not known to the controller; {tail}"),
        ));
    };
    let dest = cx.repo_cache(name)?;
    if !dest.join(".git").exists() {
        let failed = |e: &dyn std::fmt::Display| {
            CommandError::new(
                ErrorKind::Failed,
                format!("cannot clone repo {name} from {}: {e}", entry.url),
            )
        };
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| failed(&e))?;
        }
        cx.git()
            .clone_repo(&entry.url, &dest)
            .map_err(|e| failed(&e))?;
    }
    let mut eff = req.clone();
    if let (Some(branch), None) = (&entry.default_branch, &req.base) {
        let sha = cx.git().fetch_branch(&dest, branch).map_err(|e| {
            CommandError::new(
                ErrorKind::Failed,
                format!("cannot fetch {branch} of repo {name}: {e}"),
            )
        })?;
        eff.base = Some(sha);
    }
    Ok((dest, eff))
}
