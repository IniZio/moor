//! The base commit a new Box starts from.

use std::path::Path;

use moor_core::config::{parse_repo, BoxSettings, Flags};
use moor_core::{ErrorKind, StartReq, Warning};

use super::super::common::facts::{resolved, settings};
use crate::cx::Cx;
use crate::error::CommandError;

/// The repo config file name.
const REPO_FILE: &str = ".moor.yaml";

/// Settings for an unbound Box, taking the repo config from `base` because the machine checks that out.
///
/// A host `.moor.yaml` that differs from the base copy warns and is not used. With no base copy,
/// the checkout's own file is read.
pub(super) fn settings_at_base(
    cx: &Cx,
    repo: &Path,
    base: &str,
    flags: &Flags,
) -> Result<BoxSettings, CommandError> {
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

/// The base commit recorded for the Box.
///
/// An explicit base is resolved to its commit, so a branch that moves later cannot change the Box's base.
///
/// Without an explicit base the remote default branch is fetched first and its commit is the
/// base, so the Box branch tracks nothing. A failed fetch warns and leaves the local HEAD commit
/// as the base.
pub(super) fn resolve(cx: &Cx, req: &StartReq, repo: &Path) -> Result<String, CommandError> {
    if let Some(b) = &req.base {
        let sha = cx
            .git()
            .commit_of(repo, b)
            .map_err(|e| CommandError::new(ErrorKind::Usage, format!("unknown base {b}: {e}")))?;
        return Ok(sha);
    }
    let fetched = cx.git().fetch_default(repo);
    if let Ok(Some((_, sha))) = fetched {
        return Ok(sha);
    }
    let head = cx
        .git()
        .head(repo)
        .map_err(|e| CommandError::new(ErrorKind::Failed, e))?;
    if let Err(e) = fetched {
        cx.warn(Warning {
            code: "fetch_failed".to_owned(),
            message: format!(
                "could not fetch the default branch ({e}); the Box starts from local HEAD {head}, \
                 which must be on the remote"
            ),
        });
    }
    Ok(head)
}
