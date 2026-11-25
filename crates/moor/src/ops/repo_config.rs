//! `repo-config-get` and `repo-config-set`: a repo's config or Proxy rules as the store keeps
//! them.
//!
//! The operator reads and writes any repo. A user reads a repo their GitHub account can see and
//! writes one it can push to, as the account's own token says, the same check a start makes.

use moor_core::config::parse_repo;
use moor_core::config::proxy::{parse_rules_text, DEFAULT_RULES_TEXT};
use moor_core::request::{RepoConfigGetReq, RepoConfigSetReq, RepoFile};
use moor_core::seam::forge::ForgeRepo;
use moor_core::user::Viewer;
use moor_core::view::repo_config::RepoConfigValue;
use moor_core::ErrorKind;

use super::common::view::rfc3339;
use super::start::repo;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::repo_config::{RepoConfigRow, Wrote};

/// The most text a repo's config may hold, as much as a repo config may hold.
const TEXT_CAP: usize = 64 * 1024;

/// What `repo-config-get` and `repo-config-set` check about the caller.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Need {
    /// The repo is visible to the caller's GitHub account.
    See,
    /// The caller's GitHub account may push to the repo.
    Push,
}

/// `repo-config-get`: the stored text of the repo, or revision 0 when it has none: no text for
/// its config, the default rules text for its rules.
///
/// # Errors
/// Refused when the caller's account cannot see the repo; a store or forge failure.
pub(crate) fn get(cx: &Cx, req: &RepoConfigGetReq) -> Result<RepoConfigValue, CommandError> {
    let who = check(cx, &req.repo, Need::See)?;
    let mut read = value(
        &req.repo,
        req.file,
        cx.store()?.repo_text(&req.repo, req.file)?,
    );
    if req.file == RepoFile::Rules && read.text.is_none() {
        read.text = Some(DEFAULT_RULES_TEXT.to_owned());
    }
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
            format!("the {} is over {TEXT_CAP} bytes", noun(req.file)),
        ));
    }
    match req.file {
        RepoFile::Config => parse_repo(&format!("{}:repo config", req.repo), &req.text).map(drop),
        RepoFile::Rules => parse_rules_text(&format!("{}:rules", req.repo), &req.text).map(drop),
    }
    .map_err(|e| CommandError::new(ErrorKind::Usage, e.to_string()))?;
    match cx
        .store()?
        .set_repo_text(&req.repo, req.file, &req.text, &who, req.expected_revision)?
    {
        Wrote::Written(row) => Ok(RepoConfigValue {
            can_edit: true,
            ..value(&req.repo, req.file, Some(row))
        }),
        Wrote::Stale { current } => {
            let what = noun(req.file);
            let now = current.map_or_else(|| format!("no {what}"), |r| format!("revision {r}"));
            let wanted = req.expected_revision.unwrap_or_default();
            Err(CommandError::new(
                ErrorKind::Refused,
                format!(
                    "the {what} of {} is at {now}, not revision {wanted}: it changed after you read it",
                    req.repo
                ),
            )
            .with_hint(format!("read it again with `moor repo {what} get`, apply your change to that text, and set it with that revision")))
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
            format!("your GitHub account cannot push to {repo}, so it cannot change its config or rules"),
        ));
    }
    Ok(Caller {
        label,
        can_push: at.can_push,
    })
}

/// What `file` is called in messages and in `moor repo <noun>`.
const fn noun(file: RepoFile) -> &'static str {
    match file {
        RepoFile::Config => "config",
        RepoFile::Rules => "rules",
    }
}

/// The view of `row`, the repo's stored `file`.
fn value(repo: &ForgeRepo, file: RepoFile, row: Option<RepoConfigRow>) -> RepoConfigValue {
    RepoConfigValue {
        repo: repo.clone(),
        file,
        revision: row.as_ref().map_or(0, |r| r.revision),
        edited_by: row.as_ref().map(|r| r.edited_by.clone()),
        edited_at: row.as_ref().map(|r| rfc3339(r.edited_at)),
        text: row.map(|r| r.text),
        can_edit: false,
    }
}

#[cfg(test)]
mod tests;
