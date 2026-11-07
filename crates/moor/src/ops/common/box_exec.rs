//! The Box exec environment of the ops: the Proxy and the environment for one exec.

use moor_core::config::BoxSettings;
use moor_core::exec_env::{self, BoxCtx, ExecEnv, ProxyCtx, Purpose, RUN_DEADLINE};
use moor_core::seam::agent_adapter::ProxyAddr;
use moor_core::seam::forge::ForgeRepo;
use moor_core::seam::proc::Deadline;
use moor_core::seam::proxy_host::ProxySpec;
use moor_core::user::UserId;
use moor_core::{BoxHandle, BoxId, BoxPath};

use crate::cx::Cx;
use crate::ops::common::proxy::{box_env, plan, ProxyPlan};
use crate::store::BoxRow;

/// The Box's running Proxy, as a Box exec needs it.
pub(crate) struct BoxProxy {
    /// The Proxy's address and credentials inside the Box.
    pub(crate) addr: ProxyAddr,
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
}

/// Starts or reuses the Box's Proxy for `cfg`'s egress, acting for the user in the Box's row.
///
/// # Errors
/// Returns a message when the plan cannot be made or the Proxy cannot start.
pub(crate) fn proxy(
    cx: &Cx,
    box_id: BoxId,
    handle: Option<BoxHandle>,
    repo: Option<&ForgeRepo>,
    cfg: &BoxSettings,
) -> Result<BoxProxy, String> {
    let (user, plan) = box_plan(cx, box_id, repo)?;
    let repo_env: Vec<(String, String)> = cfg.env.clone().into_iter().collect();
    let spec = ProxySpec {
        box_id,
        box_handle: handle,
        box_repo: plan.repos.first().cloned(),
        other_repos: plan.repos.get(1..).unwrap_or_default().to_vec(),
        rules: plan.rules,
        user,
    };
    let rule_env = box_env(&spec.rules);
    let service = cx.config().machines.service.iter().map(|s| s.host.clone());
    let hosts = spec
        .rules
        .iter()
        .flat_map(|r| r.hosts.clone())
        .chain(service)
        .collect();
    let ep = cx.proxy_host().ensure(&spec).map_err(|e| e.to_string())?;
    let addr = ProxyAddr {
        box_port: ep
            .forward_port
            .ok_or("the Proxy gave the Box no egress forwarder port")?,
        egress_port: ep.egress_port,
        ca_pem: ep.ca_pem,
        box_env: exec_env::merged_env(repo_env, rule_env.clone()),
        hosts,
    };
    Ok(BoxProxy { addr, rule_env })
}

/// The user of Box `box_id` and the rules its Proxy would run.
fn box_plan(
    cx: &Cx,
    box_id: BoxId,
    repo: Option<&ForgeRepo>,
) -> Result<(UserId, ProxyPlan), String> {
    let (user, others) = requester(cx, box_id)?;
    let mcp = crate::ops::users::mcp::for_box(cx, Some(user)).map_err(|e| e.error.to_string())?;
    // A Box with a profile fetches its profile checkout even when it has no repo.
    let has_profile = cx
        .store()
        .map_err(|e| e.error.to_string())?
        .box_profile(box_id)
        .map_err(|e| e.to_string())?
        .is_some();
    let repos: Vec<ForgeRepo> = repo.into_iter().cloned().chain(others).collect();
    Ok((user, plan(cx, &repos, &mcp.servers, has_profile)?))
}

/// The env the steps after setup run with on Box `row`, in a steady order. It holds no secret, as
/// the Proxy places those. With rules that do not resolve it is the repo's alone, and such a Box's
/// Proxy fails before it reaches those steps.
pub(crate) fn setup_env(cx: &Cx, row: &BoxRow, cfg: &BoxSettings) -> Vec<(String, String)> {
    let repo = row.repo.as_ref().map(|r| &r.repo);
    let rules = box_plan(cx, row.id, repo).map_or_else(|_| Vec::new(), |(_, p)| box_env(&p.rules));
    let mut env = exec_env::merged_env(cfg.env.clone().into_iter().collect(), rules);
    env.sort_unstable();
    env
}

/// The user the Box acts for and the repos it lists after its primary, read from its row.
fn requester(cx: &Cx, box_id: BoxId) -> Result<(UserId, Vec<ForgeRepo>), String> {
    let row = cx
        .store()
        .map_err(|e| e.error.to_string())?
        .box_row(box_id)
        .map_err(|e| e.to_string())?;
    let user = row
        .as_ref()
        .and_then(|r| r.user)
        .ok_or_else(|| format!("Box {} acts for no user", box_id.get()))?;
    let others = row
        .and_then(|r| r.repo)
        .map(|r| r.others.into_iter().map(|l| l.repo).collect())
        .unwrap_or_default();
    Ok((user, others))
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
        proxy: proxy.map(|p| ProxyCtx::of(&p.addr)),
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
