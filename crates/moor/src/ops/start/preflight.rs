//! The credential preflight of `start`: runs before any side effect.

use std::collections::BTreeSet;
use std::path::Path;

use moor_core::config::BoxSettings;
use moor_core::domain::event::CheckedCred;
use moor_core::user::UserId;
use moor_core::{ErrorKind, StartReq};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::creds::unresolved;
use crate::store::BoxRow;

/// Why the Box must be rejected for `user`, naming each unresolved item; `None` when all
/// resolve.
///
/// # Errors
/// The store's failure to list the user's secrets.
pub(super) fn rejection(
    cx: &Cx,
    settings: &BoxSettings,
    worktree: &Path,
    user: UserId,
) -> Result<Option<String>, CommandError> {
    let have: BTreeSet<_> = cx.store()?.user_secret_names(user)?.into_iter().collect();
    let failing = unresolved(settings, worktree, &have);
    if failing.is_empty() {
        return Ok(None);
    }
    let lines: Vec<String> = failing
        .iter()
        .map(|item| {
            item.error
                .as_ref()
                .map_or_else(|| item.name.clone(), |e| format!("{}: {e}", item.name))
        })
        .collect();
    Ok(Some(format!("unresolved: {}", lines.join("; "))))
}

/// The credentials the Box uses, all resolved; names and layers only.
pub(super) fn checked(settings: &BoxSettings) -> Vec<CheckedCred> {
    settings
        .credentials
        .iter()
        .map(|c| CheckedCred {
            name: c.name.clone(),
            source: c.layer.as_str().to_owned(),
            resolves: true,
        })
        .collect()
}

/// Refuses an open Box that acts for a user other than `user`.
pub(super) fn check_owner(row: &BoxRow, user: UserId) -> Result<(), CommandError> {
    if row.user == Some(user) {
        return Ok(());
    }
    Err(CommandError::new(
        ErrorKind::Refused,
        format!("Box {} acts for another user", row.name.as_str()),
    ))
}

/// Refuses a `--worktree` that differs from the open Box's worktree.
pub(super) fn check_bound(cx: &Cx, req: &StartReq, row: &BoxRow) -> Result<(), CommandError> {
    let (Some(want), Some(have)) = (&req.worktree, &row.worktree) else {
        return Ok(());
    };
    let want = super::resolve(cx, want);
    let same = want == Path::new(have)
        || matches!((want.canonicalize(), Path::new(have).canonicalize()),
            (Ok(a), Ok(b)) if a == b);
    if same {
        return Ok(());
    }
    Err(CommandError::new(
        ErrorKind::Refused,
        format!(
            "Box {} is already bound to {have}, not {}",
            req.name.as_str(),
            want.display()
        ),
    ))
}
