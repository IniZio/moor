//! Facts every Box op needs, built once per command.

use moor_core::config::{parse_repo, BoxSettings, Flags, RepoConfig, RepoOverride};
use moor_core::domain::place::{BoxKind, PlaceState};
use moor_core::seam::driver::DriverCaps;
use moor_core::seam::forge::ForgeRepo;
use moor_core::ProviderId;

use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::BoxRow;

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

/// The repo config of `row`'s repo with the Box's own override laid over it, parsed; `None` for
/// a Box whose repo has none and that has no override.
///
/// # Errors
/// A stored text that does not parse.
pub(crate) fn repo_config(cx: &Cx, row: &BoxRow) -> Result<Option<RepoConfig>, CommandError> {
    let over = cx.store()?.box_config_override(row.id)?;
    repo_config_of(cx, task_repo(row), over.as_deref())
}

/// The repo whose config Box `row` takes: its own, else the task repo of the claim that took it.
const fn task_repo(row: &BoxRow) -> Option<&ForgeRepo> {
    match (&row.repo, &row.kind) {
        (Some(r), _) => Some(&r.repo),
        (None, BoxKind::Place(PlaceState::Claimed(claim))) => Some(&claim.repo),
        (None, _) => None,
    }
}

/// The repo config of `repo` as the store holds it now, parsed, with `over` laid over it, so an
/// edit reaches Boxes already running. A repo with none stored has an empty config, which `over`
/// still lays over. An error names the text it came from.
///
/// # Errors
/// A text that does not parse, or a store failure.
pub(crate) fn repo_config_of(
    cx: &Cx,
    repo: Option<&ForgeRepo>,
    over: Option<&str>,
) -> Result<Option<RepoConfig>, CommandError> {
    let stored = stored_config_of(cx, repo)?;
    let Some(text) = over.filter(|t| !t.trim().is_empty()) else {
        return Ok(stored);
    };
    let whose = repo.map_or_else(|| "Box".to_owned(), ToString::to_string);
    let over = RepoOverride::parse(&format!("{whose}:box config"), text)?;
    Ok(Some(over.over(stored.unwrap_or_default())))
}

/// The config the store holds for `repo`, before any override.
fn stored_config_of(cx: &Cx, repo: Option<&ForgeRepo>) -> Result<Option<RepoConfig>, CommandError> {
    let Some(repo) = repo else {
        return Ok(None);
    };
    let Some(stored) = cx.store()?.repo_config(repo)? else {
        return Ok(None);
    };
    let file = format!("{repo}:repo config (revision {})", stored.revision);
    Ok(Some(parse_repo(&file, &stored.text)?))
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

#[cfg(test)]
mod tests;
