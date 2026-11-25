//! The Proxy plan of a Box: its repo's tool rules and the env they give the Box.

use moor_core::config::check::agent_env_refused;
use moor_core::config::proxy::{
    default_rules, parse_rules_text, resolve_rules_over, ResolvedRule, ToolRule,
};
use moor_core::request::RepoFile;
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
/// a Box with no repo keeps git fetch access only when `fetch_without_repo` is set. The rules are
/// the primary repo's rules text, or the default rules text when it has none stored or the Box
/// has no repo. `extra` is the Box's own extra `allow` entries by rule id, from its repo config
/// and override: they join this Box's rules and no other Box's.
///
/// # Errors
/// The message of a rule that does not resolve.
pub(crate) fn plan(
    cx: &Cx,
    repos: &[ForgeRepo],
    mcp: &BTreeMap<String, McpServer>,
    fetch_without_repo: bool,
    extra: &BTreeMap<String, Vec<String>>,
) -> Result<ProxyPlan, String> {
    let base = repo_rules(cx, repos.first())?;
    let mut proxy = cx.config().box_proxy(mcp);
    for (name, entries) in extra {
        if !base.contains_key(name) && !proxy.rules.contains_key(name) {
            let whose = repos
                .first()
                .map_or_else(|| "the default".to_owned(), |r| format!("{r}'s"));
            return Err(format!(
                "this Box asks for extra allow entries on the rule for {name}, but {whose} rules have no such rule; add the entries to the repo's rules instead"
            ));
        }
        proxy
            .rules
            .entry(name.clone())
            .or_default()
            .allow
            .extend(entries.iter().cloned());
    }
    let rules =
        resolve_rules_over(base, &proxy, repos, fetch_without_repo).map_err(|e| e.to_string())?;
    Ok(ProxyPlan {
        repos: repos.to_vec(),
        rules,
    })
}

/// The rules of `repo`'s stored rules text by id, or the default rules when it has none.
fn repo_rules(cx: &Cx, repo: Option<&ForgeRepo>) -> Result<BTreeMap<String, ToolRule>, String> {
    let Some(repo) = repo else {
        return Ok(default_rules());
    };
    let store = cx.store().map_err(|e| e.error.to_string())?;
    match store
        .repo_text(repo, RepoFile::Rules)
        .map_err(|e| e.to_string())?
    {
        Some(row) => parse_rules_text(
            &format!("{repo}:rules (revision {})", row.revision),
            &row.text,
        )
        .map_err(|e| e.to_string()),
        None => Ok(default_rules()),
    }
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
