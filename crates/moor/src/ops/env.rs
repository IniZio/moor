//! `env`: the merged environment for a repo, with credential and MCP checks.

use moor_core::config::Flags;
use moor_core::domain::decision::BoxRepo;
use moor_core::seam::forge::RepoFiles;
use moor_core::view::env::EnvView;
use moor_core::{EnvReq, ErrorKind};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::facts::{repo_config_of, resolved};
use crate::ops::start::repo;

pub(crate) mod items;

/// Run `env`: a repo is read through the forge at `req.base` as the caller's GitHub account, as a
/// start of it would read it.
pub(crate) fn run(cx: &Cx, req: &EnvReq) -> Result<EnvView, CommandError> {
    let (shown, at) = match &req.repo {
        Some(r) => {
            let (_, user) = cx.requester()?;
            let user = user.ok_or_else(|| {
                CommandError::new(ErrorKind::Refused, "log in first: moor user enroll")
            })?;
            let at = repo::fetch(cx, user, r, req.base.as_deref(), "moor/env")?;
            let boxed = BoxRepo {
                repo: r.clone(),
                moor_yaml: at.files.moor_yaml.clone(),
                author: at.author,
            };
            (r.to_string(), Some((boxed, at.base, at.files)))
        }
        None => (String::new(), None),
    };
    let repo_cfg = match &at {
        Some((boxed, base, _)) => repo_config_of(Some(boxed), Some(base.as_str()))?,
        None => None,
    };
    let settings = resolved(cx, repo_cfg.as_ref(), &Flags::default());
    let (author, files) = match at {
        Some((boxed, _, files)) => (Some(boxed.author), files),
        None => (None, RepoFiles::default()),
    };
    Ok(EnvView::new(
        shown,
        &settings,
        items::assemble(cx, &settings, req.repo.as_ref(), author.as_ref(), &files),
    ))
}

#[cfg(test)]
mod tests;
