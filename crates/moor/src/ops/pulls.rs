//! `pulls`: the pull request on each of a Box's repos whose head is the Box's branch, read
//! from the forge as the Box's user and held for a short while, so a page that polls does not
//! ask the forge each time.

use std::iter;

use moor_core::request::BoxReq;
use moor_core::seam::forge::{BranchPull, ForgeRepo};
use moor_core::view::pulls::{PullsValue, RepoPull};
use moor_core::{BoxTarget, ErrorKind, Secret};

use super::start::repo::github_token;
use crate::cx::pulls::PullCache;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::target::{resolve, Rule};

/// Run `pulls`.
///
/// # Errors
/// Failed when no Box has the name; a store failure.
pub(crate) fn run(cx: &Cx, req: &BoxReq) -> Result<PullsValue, CommandError> {
    read_through(cx, req, &cx.pull_cache())
}

/// [`run`], with the reads of the last [`crate::cx::pulls::FRESH`] held in `cache`.
fn read_through(cx: &Cx, req: &BoxReq, cache: &PullCache) -> Result<PullsValue, CommandError> {
    let store = cx.store()?;
    let name = req.box_name.as_str();
    let row = resolve(store, &BoxTarget::Name(req.box_name.clone()), Rule::Latest)?
        .ok_or_else(|| CommandError::new(ErrorKind::Failed, format!("no Box {name}")))?;
    let Some(branch) = row.branch.clone() else {
        return Ok(PullsValue {
            branch: None,
            base: row.base_ref,
            repos: Vec::new(),
        });
    };
    let repos = row
        .repo
        .iter()
        .flat_map(|r| iter::once(&r.repo).chain(r.others.iter().map(|o| &o.repo)));
    let now = cx.clock().now();
    let mut token: Option<Result<Secret, String>> = None;
    let repos = repos
        .cloned()
        .map(|repo| {
            let key = (row.id, repo.clone());
            let read = cache.get(&key, now).unwrap_or_else(|| {
                let token = token.get_or_insert_with(|| {
                    let user = row.user.ok_or("the Box acts for no user")?;
                    github_token(cx, user).map_err(|e| e.error.to_string())
                });
                let read = read(cx, &repo, &branch, token.as_ref());
                cache.put(key, now, read.clone());
                read
            });
            let (pushed, pull, unread) = match read {
                Ok(b) => (b.pushed, b.pull, None),
                Err(why) => (false, None, Some(why)),
            };
            RepoPull {
                repo,
                pushed,
                pull,
                unread,
            }
        })
        .collect();
    Ok(PullsValue {
        branch: Some(branch),
        base: row.base_ref,
        repos,
    })
}

/// Whether `branch` is pushed in `repo` and its pull request, as `token` reads them; why they
/// could not be read.
fn read(
    cx: &Cx,
    repo: &ForgeRepo,
    branch: &str,
    token: Result<&Secret, &String>,
) -> Result<BranchPull, String> {
    let token = token.map_err(Clone::clone)?;
    cx.forge()
        .pull_of(repo, branch, token)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests;
