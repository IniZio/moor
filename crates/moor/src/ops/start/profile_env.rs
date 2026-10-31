//! The profile's `env` the agent may start with, settled against the repo's `env`.
//!
//! A profile problem never fails the start: each dropped name becomes a `profile_failed` event and
//! a warning, and the Box starts without that variable.

use moor_core::config::check::{agent_env_refused, env_name};
use moor_core::domain::event::ProfileStep;
use moor_core::profile::Manifest;
use moor_core::BoxId;

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::profile_fetch::failed;
use crate::store::Store;

/// Drops from the agent's profile `env` each name the repo's `env` also sets, since the repo's
/// value replaces the profile's and is never merged with it, and each name the repo `env` may not
/// carry either. `repo_env` is what the agent is given from the repo. Each dropped name is
/// recorded on Box `id` and warned.
///
/// # Errors
/// [`CommandError`] when an event cannot be written.
pub(super) fn settle(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    profile: Option<&mut Manifest>,
    repo_env: &[(String, String)],
) -> Result<(), CommandError> {
    let Some(profile) = profile else {
        return Ok(());
    };
    let names: Vec<String> = profile.claude.env.keys().cloned().collect();
    for name in names {
        let why = if env_name(&name).is_err() {
            "it is not a valid variable name".to_owned()
        } else if agent_env_refused(&name) {
            "it looks like a credential or is set by moor".to_owned()
        } else if repo_env.iter().any(|(n, _)| *n == name) {
            "the repo's env sets it too, and the repo's value replaces the profile's".to_owned()
        } else {
            continue;
        };
        profile.claude.env.remove(&name);
        let detail = format!("env {name} is not passed to the agent: {why}");
        failed(cx, store, id, ProfileStep::Manifest, detail)?;
    }
    Ok(())
}
