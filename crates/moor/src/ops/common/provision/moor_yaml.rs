//! Settles which `.moor.yaml` moor uses for a Box, and refuses one edited since the Box started.
//!
//! Setup re-runs, agent relaunches and `moor run` share [`guard`]; close's teardown uses
//! [`for_teardown`], which never refuses. Both return the config parsed from the exact bytes they
//! compared, so callers never read the file again after the check.

use std::path::Path;

use moor_core::config::{parse_repo, RepoConfig};
use moor_core::{ErrorKind, Warning};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::BoxRow;

/// File name of the repo config.
const REPO_FILE: &str = ".moor.yaml";

/// The worktree copy and the base-commit copy of the repo config, as read once.
struct Copies<'a> {
    /// Directory holding the worktree copy.
    at: &'a str,
    /// Base commit, or a placeholder when the Box has none.
    rev: &'a str,
    /// Worktree text; `None` when the file is absent.
    current: Option<String>,
    /// Text at the base commit; `None` when the commit has no such file.
    base: Option<String>,
}

impl Copies<'_> {
    /// Whether the worktree copy matches the base copy byte for byte.
    fn same(&self) -> bool {
        self.current == self.base
    }

    /// What differs between the two copies, in words.
    fn what(&self) -> String {
        describe(self.at, self.current.as_deref(), self.base.as_deref())
    }
}

/// Lets setup, relaunch and run use the worktree's `.moor.yaml` only when it matches the base commit.
///
/// The agent can edit that file, and these commands run what it says outside the agent. A
/// difference is refused unless the owner confirmed it with `trusted`, which warns once instead.
/// On success it returns the repo config to use, parsed from the bytes that were compared.
///
/// # Errors
/// A refused difference, an unreadable file or base commit, or a config that does not parse.
pub(crate) fn guard(
    cx: &Cx,
    row: &BoxRow,
    trusted: bool,
) -> Result<Option<RepoConfig>, CommandError> {
    let copies = read_copies(cx, row)?;
    if !copies.same() {
        let what = copies.what();
        if !trusted {
            return Err(CommandError::new(
                ErrorKind::Refused,
                format!(
                    "{}/{REPO_FILE} differs from base commit {}: {what}; moor will not run its commands unconfirmed",
                    copies.at, copies.rev
                ),
            )
            .with_hint("review the change, then repeat the command with --trust-moor-yaml"));
        }
        warn_trusted(cx, &copies, &what);
    }
    config_of(cx, &copies, copies.current.as_deref())
}

/// Like [`guard`] for close's teardown, which must not be blocked by an edited `.moor.yaml`.
///
/// A difference the owner confirmed uses the worktree copy. Otherwise it uses the base-commit copy
/// and warns once.
///
/// # Errors
/// An unreadable file or base commit, or a config that does not parse.
pub(crate) fn for_teardown(
    cx: &Cx,
    row: &BoxRow,
    trusted: bool,
) -> Result<Option<RepoConfig>, CommandError> {
    let copies = read_copies(cx, row)?;
    if copies.same() {
        return config_of(cx, &copies, copies.current.as_deref());
    }
    let what = copies.what();
    if trusted {
        warn_trusted(cx, &copies, &what);
        return config_of(cx, &copies, copies.current.as_deref());
    }
    cx.warn_once(Warning {
        code: "moor_yaml_untrusted".to_owned(),
        message: format!(
            "{}/{REPO_FILE} differs from base commit {}: {what}; using the copy at the base commit (repeat with --trust-moor-yaml to use the worktree's)",
            copies.at, copies.rev
        ),
    });
    config_of(cx, &copies, copies.base.as_deref())
}

/// Reads the worktree copy and the base-commit copy once.
fn read_copies<'a>(cx: &Cx, row: &'a BoxRow) -> Result<Copies<'a>, CommandError> {
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
    Ok(Copies {
        at,
        rev: row.base.as_deref().unwrap_or("(none)"),
        current,
        base,
    })
}

/// Parses `text`; with no text, falls back to the main checkout's file as `load_repo` would.
fn config_of(
    cx: &Cx,
    copies: &Copies<'_>,
    text: Option<&str>,
) -> Result<Option<RepoConfig>, CommandError> {
    if let Some(text) = text {
        let file = format!("{}/{REPO_FILE}", copies.at);
        return Ok(Some(parse_repo(&file, text)?));
    }
    cx.main_repo_config(Path::new(copies.at))
}

/// Warns once that the worktree copy is used because the owner confirmed it.
fn warn_trusted(cx: &Cx, copies: &Copies<'_>, what: &str) {
    cx.warn_once(Warning {
        code: "moor_yaml_trusted".to_owned(),
        message: format!(
            "{}/{REPO_FILE} differs from base commit {}: {what}; running with it as confirmed by --trust-moor-yaml",
            copies.at, copies.rev
        ),
    });
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
