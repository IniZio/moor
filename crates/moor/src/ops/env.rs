//! `env`: the merged environment for a repo, with credential and MCP checks.

use moor_core::config::egress::{egress_item, provider_applies_policy, unenforced_note};
use moor_core::config::Flags;
use moor_core::view::env::{EnvEgress, EnvView};
use moor_core::{EnvReq, ErrorKind};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::facts;

pub(crate) mod items;

/// Run `env`.
pub(crate) fn run(cx: &Cx, req: &EnvReq) -> Result<EnvView, CommandError> {
    let dir = req
        .repo
        .as_deref()
        .map_or_else(|| cx.cwd().to_path_buf(), |r| cx.cwd().join(r));
    let top = cx
        .git()
        .toplevel(&dir)
        .map_err(|e| CommandError::new(ErrorKind::Failed, e.to_string()))?;
    let settings = facts::settings(cx, &top, &Flags::default())?;
    let applies = provider_applies_policy(&settings.provider.value, settings.enforce_egress);
    let egress = egress_item(&settings.user_egress, &settings.egress, applies)?;
    let env_egress = EnvEgress {
        mode: egress.name.to_owned(),
        hosts: egress.hosts.clone(),
        note: egress.detail.clone(),
        enforce: unenforced_note(&settings.provider.value, settings.enforce_egress)
            .map(str::to_owned),
    };
    Ok(EnvView::new(
        top.display().to_string(),
        &settings,
        items::github(&settings),
        items::assemble(cx, &settings, &top, &egress),
        env_egress,
    ))
}

#[cfg(test)]
mod tests;
