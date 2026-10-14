//! The base a new Task worktree branches from.

use std::path::Path;

use moor_core::{ErrorKind, StartReq, Warning};

use crate::cx::Cx;
use crate::error::CommandError;

/// The base recorded for the Task and the ref its worktree branches from.
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
        return Ok((b.clone(), Some(b.clone())));
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
