//! A repo the controller knows: it is cloned and kept on the controller host, not read from the
//! invocation directory.

use std::fs::File;
use std::path::{Path, PathBuf};

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
    // Concurrent starts on one controller must not race the clone; held until return.
    let _lock = lock_repo(&dest)?;
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

/// Exclusive flock on `<dest>.lock`, blocking until any other holder is done.
fn lock_repo(dest: &Path) -> Result<File, CommandError> {
    let mut path = dest.as_os_str().to_owned();
    path.push(".lock");
    let path = PathBuf::from(path);
    let failed = |e: std::io::Error| {
        CommandError::new(
            ErrorKind::Failed,
            format!("cannot lock {}: {e}", path.display()),
        )
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(failed)?;
    }
    let file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(failed)?;
    file.lock().map_err(failed)?;
    Ok(file)
}
