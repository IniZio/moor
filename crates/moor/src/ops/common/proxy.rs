//! The worktree's `origin` remote, read for the proxy's per-Box upstream.

use std::path::Path;
use std::time::Duration;

use moor_core::config::check::{agent_env_reserved, credential_like_env};
use moor_core::config::proxy::{resolve_rules, ResolvedRule};
use moor_core::config::BoxSettings;
use moor_core::seam::forge::ForgeRepo;
use moor_core::seam::proc::{Deadline, ProcSpec, Tool};
use moor_core::Warning;

use crate::cx::Cx;

/// Log lines kept when a start fails.
pub(crate) const TAIL_LINES: usize = 20;

/// Allowance for reading the origin URL.
const ORIGIN_DEADLINE: Duration = Duration::from_secs(5);

/// The `origin` remote URL of the worktree, when it has one.
pub(crate) fn origin(cx: &Cx, worktree: &Path) -> Option<String> {
    let spec = ProcSpec::new(Tool::Git, Deadline(ORIGIN_DEADLINE))
        .args(["remote", "get-url", "origin"])
        .cwd(worktree);
    let out = cx.proc().run(&spec).ok()?;
    let url = out.stdout.trim();
    (!url.is_empty()).then(|| url.to_owned())
}

/// What the proxy needs to know about a Box: its repo, tool rules.
pub(crate) struct ProxyPlan {
    /// The repo `origin` names, when the forge recognises it.
    pub(crate) repo: Option<ForgeRepo>,
    /// The tool rules with `{repo}` resolved.
    pub(crate) rules: Vec<ResolvedRule>,
}

/// The repo `env` the agent may see: credential-looking and moor-set names are dropped, each with
/// an `agent_env_dropped` warning.
pub(crate) fn agent_env(cx: &Cx, settings: &BoxSettings) -> Vec<(String, String)> {
    let (kept, dropped): (Vec<_>, Vec<_>) = settings
        .env
        .iter()
        .partition(|(name, _)| !(credential_like_env(name) || agent_env_reserved(name)));
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

/// Resolves the proxy plan of a worktree whose `origin` is `origin`.
///
/// # Errors
/// The message of a rule that does not resolve.
pub(crate) fn plan(cx: &Cx, origin: Option<&str>) -> Result<ProxyPlan, String> {
    let repo = origin.and_then(|o| cx.forge().repo_of(o));
    let cfg = cx.config();
    let rules = resolve_rules(
        &cfg.proxy,
        repo.as_ref().map(|r| (r.owner.as_str(), r.name.as_str())),
        cfg.github.identity,
    )
    .map_err(|e| e.to_string())?;
    Ok(ProxyPlan { repo, rules })
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
