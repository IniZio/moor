//! `repo-config-get` and `repo-config-set`: a repo's config as the store keeps it.
//!
//! The operator reads and writes any repo. A user reads a repo their GitHub account can see and
//! writes one it can push to, as the account's own token says, the same check a start makes.

use moor_core::config::parse_repo;
use moor_core::request::{RepoConfigGetReq, RepoConfigSetReq};
use moor_core::seam::forge::ForgeRepo;
use moor_core::user::Viewer;
use moor_core::view::repo_config::RepoConfigValue;
use moor_core::ErrorKind;

use super::common::view::rfc3339;
use super::start::repo;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::repo_config::{RepoConfigRow, Wrote};

/// The most text a repo's config may hold, as much as a `.moor.yaml` read from GitHub.
const TEXT_CAP: usize = 64 * 1024;

/// What `repo-config-get` and `repo-config-set` check about the caller.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Need {
    /// The repo is visible to the caller's GitHub account.
    See,
    /// The caller's GitHub account may push to the repo.
    Push,
}

/// `repo-config-get`: the stored config of the repo, or revision 0 with no text when it has none.
///
/// # Errors
/// Refused when the caller's account cannot see the repo; a store or forge failure.
pub(crate) fn get(cx: &Cx, req: &RepoConfigGetReq) -> Result<RepoConfigValue, CommandError> {
    let who = check(cx, &req.repo, Need::See)?;
    let mut read = value(&req.repo, cx.store()?.repo_config(&req.repo)?);
    read.can_edit = who.can_push;
    Ok(read)
}

/// `repo-config-set`: replaces the repo's config with `req.text`, once it parses.
///
/// # Errors
/// Refused when the caller's account cannot push to the repo, or when `expected_revision` is not
/// the stored one; Usage for text that does not parse; a store or forge failure.
pub(crate) fn set(cx: &Cx, req: &RepoConfigSetReq) -> Result<RepoConfigValue, CommandError> {
    let who = check(cx, &req.repo, Need::Push)?.label;
    if req.text.len() > TEXT_CAP {
        return Err(CommandError::new(
            ErrorKind::Usage,
            format!("the config is over {TEXT_CAP} bytes"),
        ));
    }
    parse_repo(&format!("{}:repo config", req.repo), &req.text)
        .map_err(|e| CommandError::new(ErrorKind::Usage, e.to_string()))?;
    match cx
        .store()?
        .set_repo_config(&req.repo, &req.text, &who, req.expected_revision)?
    {
        Wrote::Written(row) => Ok(RepoConfigValue {
            can_edit: true,
            ..value(&req.repo, Some(row))
        }),
        Wrote::Stale { current } => {
            let now = current.map_or_else(|| "no config".to_owned(), |r| format!("revision {r}"));
            let wanted = req.expected_revision.unwrap_or_default();
            Err(CommandError::new(
                ErrorKind::Refused,
                format!(
                    "the config of {} is at {now}, not revision {wanted}: it changed after you read it",
                    req.repo
                ),
            )
            .with_hint("read it again with `moor repo config get`, apply your change to that text, and set it with that revision"))
        }
    }
}

/// Who a check let through.
struct Caller {
    /// The label a write records for the caller.
    label: String,
    /// Whether the caller may push to the repo, so may replace its config.
    can_push: bool,
}

/// Refuses a caller that lacks `need` on `repo`.
fn check(cx: &Cx, repo: &ForgeRepo, need: Need) -> Result<Caller, CommandError> {
    let label = cx
        .requester()
        .map_or_else(|_| "operator".to_owned(), |(who, _)| who.to_string());
    if cx.viewer() == Viewer::Operator {
        return Ok(Caller {
            label,
            can_push: true,
        });
    }
    let (_, user) = cx.requester()?;
    let user = user
        .ok_or_else(|| CommandError::new(ErrorKind::Refused, "log in first: moor user enroll"))?;
    let at = repo::fetch(cx, user, repo, None)?;
    if need == Need::Push && !at.can_push {
        return Err(CommandError::new(
            ErrorKind::Refused,
            format!("your GitHub account cannot push to {repo}, so it cannot change its config"),
        ));
    }
    Ok(Caller {
        label,
        can_push: at.can_push,
    })
}

/// The view of `row`, the repo's stored config.
fn value(repo: &ForgeRepo, row: Option<RepoConfigRow>) -> RepoConfigValue {
    RepoConfigValue {
        repo: repo.clone(),
        revision: row.as_ref().map_or(0, |r| r.revision),
        edited_by: row.as_ref().map(|r| r.edited_by.clone()),
        edited_at: row.as_ref().map(|r| rfc3339(r.edited_at)),
        text: row.map(|r| r.text),
        can_edit: false,
    }
}

#[cfg(test)]
mod tests;
