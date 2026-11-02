//! The Proxy plan of a Box: its repo's tool rules and the env they give the Box.

use moor_core::config::check::agent_env_refused;
use moor_core::config::proxy::{resolve_rules, ResolvedRule};
use std::collections::BTreeMap;

use moor_core::config::{BoxSettings, McpServer};
use moor_core::seam::forge::ForgeRepo;
use moor_core::Warning;

use crate::cx::Cx;

/// Log lines kept when a start fails.
pub(crate) const TAIL_LINES: usize = 20;

/// What the proxy needs to know about a Box: its repos, tool rules.
pub(crate) struct ProxyPlan {
    /// The Box's repos, the primary first; empty when it has none.
    pub(crate) repos: Vec<ForgeRepo>,
    /// The tool rules with `{repo}` resolved.
    pub(crate) rules: Vec<ResolvedRule>,
}

/// The repo `env` the agent may see: credential-looking and moor-set names are dropped, each with
/// an `agent_env_dropped` warning.
pub(crate) fn agent_env(cx: &Cx, settings: &BoxSettings) -> Vec<(String, String)> {
    let (kept, dropped): (Vec<_>, Vec<_>) = settings
        .env
        .iter()
        .partition(|(name, _)| !agent_env_refused(name));
    for (name, _) in dropped {
        cx.warn(Warning {
            code: "agent_env_dropped".to_owned(),
            message: format!("env {name} looks like a credential or is set by moor, so it is not passed to the agent"),
        });
    }
    kept.into_iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// Resolves the proxy plan of a Box of `repos` (the primary first) with the MCP servers `mcp`;
/// a Box with no repo keeps git fetch access only when `fetch_without_repo` is set.
///
/// # Errors
/// The message of a rule that does not resolve.
pub(crate) fn plan(
    cx: &Cx,
    repos: &[ForgeRepo],
    mcp: &BTreeMap<String, McpServer>,
    fetch_without_repo: bool,
) -> Result<ProxyPlan, String> {
    let rules = resolve_rules(&cx.config().box_proxy(mcp), repos, fetch_without_repo)
        .map_err(|e| e.to_string())?;
    Ok(ProxyPlan {
        repos: repos.to_vec(),
        rules,
    })
}

/// The non-secret Box env of `rules` in rule order; a later rule's value replaces an earlier one.
pub(crate) fn box_env(rules: &[ResolvedRule]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for (k, v) in rules.iter().flat_map(|r| &r.box_env) {
        match out.iter_mut().find(|(name, _)| name == k) {
            Some(slot) => slot.1.clone_from(v),
            None => out.push((k.clone(), v.clone())),
        }
    }
    out
}
