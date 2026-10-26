//! The credential preflight of `start`: runs before any side effect.

use std::collections::BTreeSet;

use moor_core::config::BoxSettings;
use moor_core::domain::event::CheckedCred;
use moor_core::seam::forge::RepoFiles;
use moor_core::user::UserId;
use moor_core::ErrorKind;

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::authz;
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
    files: &RepoFiles,
    user: UserId,
) -> Result<Option<String>, CommandError> {
    let have: BTreeSet<_> = cx.store()?.user_secret_names(user)?.into_iter().collect();
    let failing = unresolved(settings, files, &have);
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

/// The `box_credentials` the Box requires, all resolved; names and layers only. A missing MCP
/// secret leaves its server out instead.
pub(super) fn checked(settings: &BoxSettings) -> Vec<CheckedCred> {
    settings
        .credentials
        .iter()
        .filter(|c| c.boxed)
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
    Err(CommandError::new(ErrorKind::Refused, authz::NOT_YOURS))
}
