//! `env`: the merged environment for a repo, with credential and MCP checks.

use moor_core::config::Flags;
use moor_core::view::env::EnvView;
use moor_core::{EnvReq, ErrorKind};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::facts;
use crate::ops::common::host_path::{absolute, caller_dir};

pub(crate) mod items;

/// Run `env`.
pub(crate) fn run(cx: &Cx, req: &EnvReq) -> Result<EnvView, CommandError> {
    let dir = match req.repo.as_deref() {
        Some(repo) => absolute("repo", repo)?,
        None => caller_dir(cx)?.to_path_buf(),
    };
    let top = cx
        .git()
        .toplevel(&dir)
        .map_err(|e| CommandError::new(ErrorKind::Failed, e.to_string()))?;
    let settings = facts::settings(cx, &top, &Flags::default())?;
    Ok(EnvView::new(
        top.display().to_string(),
        &settings,
        items::assemble(cx, &settings, &top),
    ))
}

#[cfg(test)]
mod tests;
