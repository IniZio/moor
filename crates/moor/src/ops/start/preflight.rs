//! The credential preflight of `start`: runs before any side effect.

use std::path::Path;

use moor_core::config::TaskSettings;
use moor_core::domain::event::CheckedCred;

use crate::cx::Cx;
use crate::ops::common::creds::unresolved;

/// Why the Task must be rejected, naming each unresolved item; `None` when all resolve.
pub(super) fn rejection(cx: &Cx, settings: &TaskSettings, worktree: &Path) -> Option<String> {
    let failing = unresolved(cx, settings, worktree);
    if failing.is_empty() {
        return None;
    }
    let lines: Vec<String> = failing
        .iter()
        .map(|item| {
            item.error
                .as_ref()
                .map_or_else(|| item.name.clone(), |e| format!("{}: {e}", item.name))
        })
        .collect();
    Some(format!("unresolved: {}", lines.join("; ")))
}

/// The credentials the Task uses, all resolved; names and layers only.
pub(super) fn checked(settings: &TaskSettings) -> Vec<CheckedCred> {
    settings
        .used_credentials()
        .map(|c| CheckedCred {
            name: c.name.clone(),
            source: c.layer.as_str().to_owned(),
            resolves: true,
        })
        .collect()
}
