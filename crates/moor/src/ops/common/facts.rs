//! Facts every Box op needs, built once per command.

use moor_core::config::{parse_repo, BoxSettings, Flags, RepoConfig};
use moor_core::domain::decision::BoxRepo;
use moor_core::seam::driver::DriverCaps;
use moor_core::ProviderId;

use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::BoxRow;
use moor_core::ErrorKind;

/// Settings for one command on `row`: user config from Cx, the repo's config as the store holds
/// it now, then flags.
///
/// # Errors
/// A stored repo config that no longer parses.
pub(crate) fn settings_of(
    cx: &Cx,
    row: &BoxRow,
    flags: &Flags,
) -> Result<BoxSettings, CommandError> {
    Ok(resolved(cx, repo_config(cx, row)?.as_ref(), flags))
}

/// The repo config of `row`'s repo, parsed; `None` for a Box whose repo has none.
///
/// # Errors
/// A stored text that does not parse.
pub(crate) fn repo_config(cx: &Cx, row: &BoxRow) -> Result<Option<RepoConfig>, CommandError> {
    repo_config_of(cx, row.repo.as_ref(), row.base.as_deref())
}

/// The repo config of `repo`, parsed. A config stored for the repo wins, so an edit reaches
/// Boxes already running; a repo with none uses the `.moor.yaml` read at `base`. An error names
/// the text it came from.
///
/// # Errors
/// A text that does not parse, or a store failure.
pub(crate) fn repo_config_of(
    cx: &Cx,
    repo: Option<&BoxRepo>,
    base: Option<&str>,
) -> Result<Option<RepoConfig>, CommandError> {
    let Some(repo) = repo else {
        return Ok(None);
    };
    if let Some(stored) = cx.store()?.repo_config(&repo.repo)? {
        let file = format!("{}:repo config (revision {})", repo.repo, stored.revision);
        return Ok(Some(parse_repo(&file, &stored.text)?));
    }
    let Some(text) = repo.moor_yaml.as_deref() else {
        return Ok(None);
    };
    let file = format!("{}@{}:.moor.yaml", repo.repo, base.unwrap_or("base"));
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
