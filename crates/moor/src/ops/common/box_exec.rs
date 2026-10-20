//! The Box exec environment of the ops: the Proxy and the environment for one exec.

use std::path::Path;

use moor_core::config::proxy::{check_direct, ResolvedRule, FORGE_CREDENTIAL};
use moor_core::config::BoxSettings;
use moor_core::exec_env::{self, BoxCtx, ExecEnv, ProxyCtx, Purpose, RUN_DEADLINE};
use moor_core::seam::agent_profile::ProxyAddr;
use moor_core::seam::proc::Deadline;
use moor_core::seam::proxy_host::ProxySpec;
use moor_core::{BoxHandle, BoxId, BoxPath};

use crate::cx::Cx;
use crate::ops::common::proxy::{box_env, origin, plan};

/// The Box's running Proxy, as a Box exec needs it.
pub(crate) struct BoxProxy {
    /// The Proxy's address and credentials inside the Box.
    pub(crate) addr: ProxyAddr,
    /// The git origin URL of the worktree, when it has one.
    pub(crate) origin: Option<String>,
    /// The non-secret env of the tool rules, which wins over the repo `env`.
    pub(crate) rule_env: Vec<(String, String)>,
}

/// The Box an exec runs in and where it runs.
pub(crate) struct Site<'a> {
    /// The Box.
    pub(crate) handle: &'a BoxHandle,
    /// The Box's provider, which names the variables to strip.
    pub(crate) provider: &'a str,
    /// The directory commands run in; `None` keeps the driver's default.
    pub(crate) workdir: Option<&'a BoxPath>,
    /// The host worktree of the Box.
    pub(crate) worktree: &'a Path,
}

/// Starts or reuses the Box's Proxy for `cfg`'s egress.
///
/// # Errors
/// Returns a message when the plan cannot be made or the Proxy cannot start.
pub(crate) fn proxy(
    cx: &Cx,
    box_id: BoxId,
    handle: Option<BoxHandle>,
    worktree: &Path,
    cfg: &BoxSettings,
) -> Result<BoxProxy, String> {
    let origin = origin(cx, worktree);
    let plan = plan(cx, origin.as_deref())?;
    let repo_env: Vec<(String, String)> = cfg.env.clone().into_iter().collect();
    let spec = ProxySpec {
        box_id,
        box_handle: handle,
        host_cwd: worktree.to_path_buf(),
        box_repo: plan.repo,
        rules: plan.rules,
    };
    check_direct(
        &cfg.direct,
        "proxy.direct",
        &spec
            .rules
            .iter()
            .flat_map(|r| r.hosts.iter().map(String::as_str))
            .collect::<Vec<_>>(),
    )
    .map_err(|e| e.to_string())?;
    let rule_env = box_env(&spec.rules);
    let forge = forge_routed(&spec.rules);
    let ep = cx.proxy_host().ensure(&spec).map_err(|e| e.to_string())?;
    let addr = ProxyAddr {
        box_port: ep.box_port,
        ca_pem: ep.ca_pem,
        box_env: exec_env::merged_env(repo_env, rule_env.clone()),
        forge,
        direct: cfg.direct.clone(),
    };
    Ok(BoxProxy {
        addr,
        origin,
        rule_env,
    })
}

/// Whether a rule sends GitHub through the Proxy with the Box's forge token.
fn forge_routed(rules: &[ResolvedRule]) -> bool {
    rules.iter().any(|r| r.credential == FORGE_CREDENTIAL)
}

/// The variable names the `provider`'s platform puts in the Box that moor removes.
pub(crate) fn strip_env(cx: &Cx, provider: &str) -> Vec<String> {
    cx.config()
        .drivers
        .get(provider)
        .map_or_else(Vec::new, |d| d.strip_env.clone())
}

/// A Box exec environment from `exec_env`.
pub(crate) struct BoxExec {
    /// The environment the exec runs with.
    pub(crate) env: ExecEnv,
}

/// Builds the environment for `purpose`. With a Proxy its prelude writes the Proxy CA inside the
/// user's command exec, so no step runs on the Box before that exec.
///
/// # Errors
/// Returns a message when the environment cannot be built.
pub(crate) fn prepare(
    cx: &Cx,
    site: &Site<'_>,
    proxy: Option<&BoxProxy>,
    cfg: &BoxSettings,
    purpose: Purpose,
) -> Result<BoxExec, String> {
    let strip = strip_env(cx, site.provider);
    let repo_env: Vec<(String, String)> = cfg.env.clone().into_iter().collect();
    let rule_env = proxy.map(|p| p.rule_env.clone()).unwrap_or_default();
    let env = exec_env::merged_env(repo_env, rule_env);
    let ctx = BoxCtx {
        env: &env,
        proxy: proxy.map(|p| ProxyCtx {
            box_port: p.addr.box_port,
            ca_pem: &p.addr.ca_pem,
            direct: &p.addr.direct,
        }),
        origin: proxy.and_then(|p| p.origin.as_deref()),
        workdir: site.workdir,
        strip: &strip,
    };
    exec_env::build(&ctx, purpose).map(|env| BoxExec { env })
}

impl BoxExec {
    /// `argv` wrapped so the prelude runs first.
    pub(crate) fn argv(&self, argv: Vec<String>) -> Vec<String> {
        exec_env::wrap_argv(&self.env, argv)
    }

    /// The non-secret variables to pass as the exec's env.
    pub(crate) fn vars(&self) -> Vec<(String, String)> {
        self.env.vars.clone()
    }

    /// The directory to run in; `None` keeps the driver's default.
    pub(crate) fn cwd(&self) -> Option<BoxPath> {
        self.env.cwd.clone()
    }

    /// The wall-clock limit of the exec.
    pub(crate) fn deadline(&self) -> Deadline {
        self.env.deadline.unwrap_or(Deadline(RUN_DEADLINE))
    }
}

#[cfg(test)]
mod tests;
