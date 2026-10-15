//! Refuses work outside the sandbox that would use a `.moor.yaml` edited since the Task started.
//!
//! Setup re-runs, agent relaunches and `moor run` all share this check.

use std::path::Path;

use moor_core::config::parse_repo;
use moor_core::{ErrorKind, Warning};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::TaskRow;

/// File name of the repo config.
const REPO_FILE: &str = ".moor.yaml";

/// Lets setup, relaunch and run use the worktree's `.moor.yaml` only when it matches the base commit.
///
/// The agent can edit that file, and these commands run work outside the agent sandbox. A
/// difference is refused unless the owner confirmed it with `trusted`, which warns once instead.
pub(crate) fn guard(cx: &Cx, row: &TaskRow, trusted: bool) -> Result<(), CommandError> {
    let at = row.worktree.as_deref().unwrap_or(&row.repo);
    let path = Path::new(at).join(REPO_FILE);
    let current = match std::fs::read_to_string(&path) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            return Err(CommandError::new(
                ErrorKind::Failed,
                format!("cannot read {}: {e}", path.display()),
            ))
        }
    };
    let base = match row.base.as_deref() {
        Some(rev) => cx
            .git()
            .file_at(Path::new(&row.repo), rev, REPO_FILE)
            .map_err(|e| {
                CommandError::new(
                    ErrorKind::Failed,
                    format!("cannot read {REPO_FILE} at base commit {rev}: {e}"),
                )
            })?,
        None => None,
    };
    if current == base {
        return Ok(());
    }
    let what = describe(at, current.as_deref(), base.as_deref());
    let rev = row.base.as_deref().unwrap_or("(none)");
    if trusted {
        cx.warn_once(Warning {
            code: "moor_yaml_trusted".to_owned(),
            message: format!(
                "{at}/{REPO_FILE} differs from base commit {rev}: {what}; running with it as confirmed by --trust-moor-yaml"
            ),
        });
        return Ok(());
    }
    Err(CommandError::new(
        ErrorKind::Refused,
        format!(
            "{at}/{REPO_FILE} differs from base commit {rev}: {what}; moor will not run work outside the sandbox with it unconfirmed"
        ),
    )
    .with_hint("review the change, then repeat the command with --trust-moor-yaml"))
}

/// What differs between the worktree copy and the base copy, in words.
fn describe(at: &str, current: Option<&str>, base: Option<&str>) -> String {
    match (current, base) {
        (Some(now), Some(then)) => {
            let file = format!("{at}/{REPO_FILE}");
            match (parse_repo(&file, now), parse_repo(&file, then)) {
                (Ok(a), Ok(b)) => {
                    let keys = a.changed_keys(&b);
                    if keys.is_empty() {
                        "formatting only".to_owned()
                    } else {
                        keys.join(", ")
                    }
                }
                _ => "the whole file".to_owned(),
            }
        }
        (Some(_), None) => "added".to_owned(),
        (None, Some(_)) => "deleted".to_owned(),
        (None, None) => String::new(),
    }
}
