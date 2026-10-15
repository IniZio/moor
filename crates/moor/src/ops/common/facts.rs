//! Facts every task op needs, built once per command.

use std::path::Path;

use moor_core::config::egress::{effective_allow_list, provider_applies_policy};
use moor_core::config::{Flags, TaskSettings};
use moor_core::seam::driver::{EgressPlan, EgressSupport};
use moor_core::ProviderId;

use crate::cx::Cx;
use crate::error::CommandError;
use moor_core::ErrorKind;

/// Settings for one command: user config from Cx, repo config worktree-first, flags.
///
/// # Errors
/// A bad repo config or an unresolvable setting, as a failed command.
pub(crate) fn settings(
    cx: &Cx,
    worktree: &Path,
    flags: &Flags,
) -> Result<TaskSettings, CommandError> {
    let repo = cx.repo_config(worktree)?;
    Ok(TaskSettings::resolve(cx.config(), repo.as_ref(), flags))
}

/// How far the settings' provider can restrict Box egress.
///
/// # Errors
/// The driver cannot report the provider's capabilities.
pub(crate) fn egress_support(cx: &Cx, cfg: &TaskSettings) -> Result<EgressSupport, CommandError> {
    let provider = ProviderId::new(cfg.provider.value.clone());
    Ok(cx.driver().caps(&provider)?.egress)
}

/// The egress plan the settings call for.
///
/// # Errors
/// `box.enforce_egress` on a provider that cannot enforce egress, or a repo `egress` entry that
/// is not a valid domain.
pub(crate) fn egress_plan(
    agent: &[&str],
    cfg: &TaskSettings,
    support: EgressSupport,
) -> Result<EgressPlan, CommandError> {
    if support == EgressSupport::None && cfg.enforce_egress {
        let provider = &cfg.provider.value;
        return Err(CommandError::new(
            ErrorKind::Refused,
            format!("provider {provider} cannot enforce egress, but box.enforce_egress is set"),
        )
        .with_hint("unset box.enforce_egress, or choose a provider with egress support"));
    }
    if provider_applies_policy(support, cfg.enforce_egress) {
        Ok(EgressPlan::Allow(effective_allow_list(
            agent,
            &cfg.user_egress,
            &cfg.egress,
        )?))
    } else {
        Ok(EgressPlan::Open)
    }
}

#[cfg(test)]
mod tests;
