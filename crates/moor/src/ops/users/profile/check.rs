//! `profile-check`: what a profile resolves to through the forge, with nothing written.

use moor_core::name::{ForgeRepo, ProfileSource};
use moor_core::profile::{self, GithubRepo, PluginRef};
use moor_core::user::UserId;
use moor_core::view::user::{
    AgentInstalls, NamedRepo, ProfileCheckValue, ProfileLastFailed, ProfileProblem, ProfileResolved,
};
use std::collections::BTreeMap;

use super::to_value;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::start::profile::read;
use crate::store::ProfileFailedRow;

/// `profile-check`: `source`, else `user`'s recorded profile, read as `user`.
pub(super) fn run(
    cx: &Cx,
    user: UserId,
    source: Option<&ProfileSource>,
) -> Result<serde_json::Value, CommandError> {
    let store = cx.store()?;
    let source = match source {
        Some(source) => Some(source.clone()),
        None => store.user_profile(user)?,
    };
    let last_failed = store.newest_profile_failed(user)?.map(last_failed);
    let Some(source) = source else {
        return to_value(&ProfileCheckValue {
            source: None,
            resolved: None,
            failures: Vec::new(),
            last_failed,
        });
    };
    let found = read(cx, user, source.clone(), true);
    let resolved = found.resolved.map(|p| {
        let manifest = p.manifest.as_deref().and_then(|t| profile::parse(t).ok());
        let (claude, codex) = manifest.map_or_else(Default::default, |m| {
            (
                installs(m.claude.model, &m.claude.marketplaces, &m.claude.plugins),
                installs(m.codex.model, &m.codex.marketplaces, &m.codex.plugins),
            )
        });
        ProfileResolved {
            sha: p.sha.as_str().to_owned(),
            setup: found.setup.unwrap_or(false),
            claude,
            codex,
            unresolved: found.unresolved,
        }
    });
    to_value(&ProfileCheckValue {
        source: Some(source),
        resolved,
        failures: found
            .failures
            .into_iter()
            .map(|(step, detail)| ProfileProblem { step, detail })
            .collect(),
        last_failed,
    })
}

/// What one agent's section of a manifest installs.
fn installs(
    model: Option<String>,
    marketplaces: &BTreeMap<String, GithubRepo>,
    plugins: &[PluginRef],
) -> AgentInstalls {
    AgentInstalls {
        model,
        marketplaces: marketplaces
            .iter()
            .map(|(name, GithubRepo { repo, git_ref })| NamedRepo {
                name: name.clone(),
                repo: ProfileSource {
                    repo: ForgeRepo::clone(repo),
                    git_ref: git_ref.clone(),
                }
                .to_string(),
            })
            .collect(),
        plugins: plugins
            .iter()
            .map(|p| format!("{}@{}", p.name, p.marketplace))
            .collect(),
    }
}

/// The newest failure as the check shows it; a payload that is not a failure shows as unknown.
fn last_failed(row: ProfileFailedRow) -> ProfileLastFailed {
    let field = |key: &str| row.payload.get(key).cloned();
    ProfileLastFailed {
        step: field("step")
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or(moor_core::domain::event::ProfileStep::Resolve),
        detail: field("detail")
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default(),
        box_name: row.box_name,
        at_ms: row.at.as_millis(),
    }
}
