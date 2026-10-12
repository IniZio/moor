//! The credential preflight of `start`: runs before any side effect.

use std::path::Path;

use moor_core::config::TaskSettings;
use moor_core::decide::intent::{age, provisioning_dead};
use moor_core::domain::event::CheckedCred;
use moor_core::ErrorKind;

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::creds::unresolved;
use crate::store::TaskRow;

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

/// Refuses a task whose provisioning owner died before recording a Box.
pub(super) fn dead_provisioning(cx: &Cx, row: &TaskRow) -> Result<(), CommandError> {
    let (Some(intent), Some(at)) = (row.intent, row.intent_at) else {
        return Ok(());
    };
    let handle = row.boxed.as_ref().and_then(|b| b.handle.as_ref());
    let held = age(at, cx.clock().now());
    if !provisioning_dead(&intent, cx.proc().alive(intent.pid()), handle, held) {
        return Ok(());
    }
    let name = row.name.as_str();
    Err(CommandError::new(
        ErrorKind::Failed,
        format!("provisioning dead; run moor close {name}"),
    )
    .with_hint(format!("moor close {name}")))
}
