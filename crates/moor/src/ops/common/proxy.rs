//! The worktree's `origin` remote, read for the proxy's per-Task upstream.

use std::path::Path;
use std::time::Duration;

use moor_core::config::egress::effective_allow_list;
use moor_core::config::proxy::{resolve_rules, ResolvedRule};
use moor_core::config::Flags;
use moor_core::seam::forge::ForgeRepo;
use moor_core::seam::proc::{Deadline, ProcSpec, Tool};

use crate::cx::Cx;
use crate::ops::common::facts::settings;

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

/// What the proxy needs to know about a Task: its repo, tool rules and Box egress allow-list.
pub(crate) struct ProxyPlan {
    /// The repo `origin` names, when the forge recognises it.
    pub(crate) repo: Option<ForgeRepo>,
    /// The tool rules with `{repo}` resolved.
    pub(crate) rules: Vec<ResolvedRule>,
    /// The Box egress allow-list, without the rules' hosts.
    pub(crate) allow: Vec<String>,
}

/// Resolves the proxy plan of the worktree whose `origin` is `origin`.
///
/// # Errors
/// The message of a settings, rule or egress entry that does not resolve.
pub(crate) fn plan(cx: &Cx, worktree: &Path, origin: Option<&str>) -> Result<ProxyPlan, String> {
    let repo = origin.and_then(|o| cx.forge().repo_of(o));
    let cfg = cx.config();
    let rules = resolve_rules(
        &cfg.proxy,
        &cfg.credentials,
        repo.as_ref().map(|r| (r.owner.as_str(), r.name.as_str())),
    )
    .map_err(|e| e.to_string())?;
    let task = settings(cx, worktree, &Flags::default()).map_err(|e| e.error.to_string())?;
    let allow = effective_allow_list(&task.user_egress, &task.egress).map_err(|e| e.to_string())?;
    Ok(ProxyPlan { repo, rules, allow })
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
