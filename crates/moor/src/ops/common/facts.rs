//! Facts every task op needs, built once per command.

use std::path::Path;

use moor_core::config::{Flags, TaskSettings};
use moor_core::seam::driver::DriverCaps;
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

/// The capabilities of the settings' provider.
///
/// # Errors
/// The driver cannot report the provider's capabilities.
pub(crate) fn provider_caps(cx: &Cx, cfg: &TaskSettings) -> Result<DriverCaps, CommandError> {
    let provider = ProviderId::new(cfg.provider.value.clone());
    Ok(cx.driver().caps(&provider)?)
}

/// Refuses a provider that cannot carry the reverse tunnel the Box reaches the Proxy through.
///
/// Returns `caps` unchanged when they allow it.
///
/// # Errors
/// The provider's `caps` lack `reverse_tunnel`.
pub(crate) fn require_reverse_tunnel(
    cfg: &TaskSettings,
    caps: DriverCaps,
) -> Result<DriverCaps, CommandError> {
    let provider = &cfg.provider.value;
    if caps.reverse_tunnel {
        return Ok(caps);
    }
    Err(CommandError::new(
        ErrorKind::Refused,
        format!("provider {provider} lacks the reverse_tunnel capability the Box needs to reach the Proxy"),
    )
    .with_hint("choose a provider with reverse tunnel support (for crabbox, one that offers the ssh feature)"))
}

#[cfg(test)]
mod tests;
