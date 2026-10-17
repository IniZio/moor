//! The base a new Task worktree branches from.

use std::path::Path;

use moor_core::config::{parse_repo, Flags, TaskSettings};
use moor_core::{ErrorKind, StartReq, Warning};

use super::super::common::facts::{resolved, settings};
use crate::cx::Cx;
use crate::error::CommandError;

/// The repo config file name.
const REPO_FILE: &str = ".moor.yaml";

/// Settings for an unbound Task, taking the repo config from `base` because the Box checks that out.
///
/// A host `.moor.yaml` that differs from the base copy warns and is not used. With no base copy,
/// the checkout's own file is read.
pub(super) fn settings_at_base(
    cx: &Cx,
    repo: &Path,
    base: &str,
    flags: &Flags,
) -> Result<TaskSettings, CommandError> {
    let text = cx.git().file_at(repo, base, REPO_FILE).map_err(|e| {
        CommandError::new(
            ErrorKind::Failed,
            format!("cannot read {REPO_FILE} at base commit {base}: {e}"),
        )
    })?;
    let Some(text) = text else {
        return settings(cx, repo, flags);
    };
    let host = std::fs::read_to_string(repo.join(REPO_FILE)).ok();
    if host.is_some_and(|h| h != text) {
        cx.warn_once(Warning {
            code: "moor_yaml_differs_from_base".to_owned(),
            message: format!(
                "{}/{REPO_FILE} differs from base {base}; using the base copy (push it or start from that commit)",
                repo.display()
            ),
        });
    }
    let file = format!("{base}:{REPO_FILE}");
    let repo_cfg = parse_repo(&file, &text)?;
    Ok(resolved(cx, Some(&repo_cfg), flags))
}

/// The base recorded for the Task and the ref its worktree branches from.
///
/// An explicit base is resolved to its commit, so a branch that moves later cannot change the Task's base.
///
/// Without an explicit base or a bound worktree the remote default branch is fetched first and
/// its commit is the base, so the Task branch tracks nothing. A failed fetch warns and leaves the
/// local branch as the base.
pub(super) fn resolve(
    cx: &Cx,
    req: &StartReq,
    repo: &Path,
    bound: Option<&Path>,
) -> Result<(String, Option<String>), CommandError> {
    if let Some(b) = &req.base {
        let sha = cx
            .git()
            .commit_of(repo, b)
            .map_err(|e| CommandError::new(ErrorKind::Usage, format!("unknown base {b}: {e}")))?;
        return Ok((sha.clone(), Some(sha)));
    }
    if bound.is_none() {
        match cx.git().fetch_default(repo) {
            Ok(Some((_, sha))) => return Ok((sha.clone(), Some(sha))),
            Ok(None) => {}
            Err(e) => cx.warn(Warning {
                code: "fetch_failed".to_owned(),
                message: format!("could not fetch the default branch, using the local one: {e}"),
            }),
        }
    }
    let head = cx
        .git()
        .head(bound.unwrap_or(repo))
        .map_err(|e| CommandError::new(ErrorKind::Failed, e))?;
    Ok((head, None))
}
