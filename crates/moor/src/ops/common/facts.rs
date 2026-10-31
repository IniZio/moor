//! Facts every Box op needs, built once per command.

use moor_core::config::{parse_repo, BoxSettings, Flags, RepoConfig};
use moor_core::domain::decision::BoxRepo;
use moor_core::seam::driver::DriverCaps;
use moor_core::ProviderId;

use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::BoxRow;
use moor_core::ErrorKind;

/// Settings for one command on `row`: user config from Cx, the `.moor.yaml` stored when the Box
/// started, then flags.
///
/// # Errors
/// A stored repo config that no longer parses.
pub(crate) fn settings_of(
    cx: &Cx,
    row: &BoxRow,
    flags: &Flags,
) -> Result<BoxSettings, CommandError> {
    Ok(resolved(cx, repo_config(row)?.as_ref(), flags))
}

/// The `.moor.yaml` stored with `row`, parsed; `None` for a Box whose base has none.
///
/// # Errors
/// A stored text that does not parse.
pub(crate) fn repo_config(row: &BoxRow) -> Result<Option<RepoConfig>, CommandError> {
    repo_config_of(row.repo.as_ref(), row.base.as_deref())
}

/// The `.moor.yaml` of `repo` read at `base`, parsed; named `OWNER/NAME@BASE:.moor.yaml` in errors.
///
/// # Errors
/// A text that does not parse.
pub(crate) fn repo_config_of(
    repo: Option<&BoxRepo>,
    base: Option<&str>,
) -> Result<Option<RepoConfig>, CommandError> {
    let Some((repo, text)) = repo.and_then(|r| Some((&r.repo, r.moor_yaml.as_deref()?))) else {
        return Ok(None);
    };
    let file = format!("{repo}@{}:.moor.yaml", base.unwrap_or("base"));
    Ok(Some(parse_repo(&file, text)?))
}

/// Settings for one command from an already-read repo config, user config from Cx, and flags.
pub(crate) fn resolved(cx: &Cx, repo: Option<&RepoConfig>, flags: &Flags) -> BoxSettings {
    BoxSettings::resolve(cx.config(), repo, flags)
}

/// The capabilities of the settings' provider.
///
/// # Errors
/// The driver cannot report the provider's capabilities.
pub(crate) fn provider_caps(cx: &Cx, provider: &str) -> Result<DriverCaps, CommandError> {
    Ok(cx.driver().caps(&ProviderId::new(provider))?)
}

/// Refuses a provider that cannot carry the reverse tunnel the Box reaches the Proxy through.
///
/// Returns `caps` unchanged when they allow it.
///
/// # Errors
/// The provider's `caps` lack `reverse_tunnel`.
pub(crate) fn require_reverse_tunnel(
    provider: &str,
    caps: DriverCaps,
) -> Result<DriverCaps, CommandError> {
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
