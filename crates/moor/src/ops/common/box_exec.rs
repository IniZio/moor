//! The Box exec environment of the ops: the Proxy and the environment for one exec.

use std::collections::BTreeMap;
use std::time::Duration;

use moor_core::box_key::is_machine_handle;
use moor_core::config::BoxSettings;
use moor_core::domain::hands::HandsInc;
use moor_core::domain::place::{BoxKind, PlaceState};
use moor_core::exec_env::{self, BoxCtx, ExecEnv, ProxyCtx, Purpose, RUN_DEADLINE};
use moor_core::seam::agent_adapter::ProxyAddr;
use moor_core::seam::driver::{ExecOutcome, ExecSpec, Place};
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
    /// The env that points tools at their cache volumes, which the repo `env` overrides.
    pub(crate) cache_env: Vec<(String, String)>,
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
    hands: Option<(BoxHandle, HandsInc)>,
    repo: Option<&ForgeRepo>,
    cfg: &BoxSettings,
) -> Result<BoxProxy, String> {
    let (user, branch, plan) = box_plan(cx, box_id, repo, &cfg.proxy_allow)?;
    let cache_env = cfg.cache.env(repo);
    let repo_env = under_repo_env(&cache_env, cfg);
    let (box_handle, hands_inc) = hands.map_or((None, None), |(h, inc)| (Some(h), Some(inc.get())));
    let spec = ProxySpec {
        box_id,
        box_handle,
        hands_inc,
        box_repo: plan.repos.first().cloned(),
        other_repos: plan.repos.get(1..).unwrap_or_default().to_vec(),
        branch,
        rules: plan.rules,
        user,
    };
    let rule_env = box_env(&spec.rules);
    let served = cx.config().served_hosts().map(|s| s.host.clone());
    let hosts: Vec<String> = spec
        .rules
        .iter()
        .flat_map(|r| r.hosts.clone())
        .chain(served)
        .collect();
    let ep = cx.proxy_host().ensure(&spec).map_err(|e| e.to_string())?;
    let forward_port = ep
        .forward_port
        .ok_or("the Proxy gave the Box no egress forwarder port")?;
    // A machine's Box has two places that share one network, so one proxy serves both: the
    // agent place, which owns that network, keeps it running, and no exec's prelude does.
    let machine = spec
        .box_handle
        .as_ref()
        .filter(|h| is_machine_handle(h.as_str()));
    if let Some(handle) = machine {
        keep_egress(cx, handle, (ep.egress_port, forward_port), &hosts)?;
        let vars = [
            exec_env::proxy_vars(ep.egress_port),
            cache_env.clone(),
            rule_env.clone(),
        ]
        .concat();
        cx.driver()
            .set_env(handle, &vars, Some(&ep.ca_pem))
            .map_err(|e| format!("cannot give the Box its Proxy environment: {e}"))?;
    }
    let addr = ProxyAddr {
        box_port: forward_port,
        egress_port: ep.egress_port,
        ca_pem: ep.ca_pem,
        box_env: exec_env::merged_env(repo_env, rule_env.clone()),
        hosts,
        keep_egress: machine.is_none(),
    };
    Ok(BoxProxy {
        addr,
        rule_env,
        cache_env,
    })
}

/// The repo `env`, then the variables of `cache_env` it does not set.
fn under_repo_env(cache_env: &[(String, String)], cfg: &BoxSettings) -> Vec<(String, String)> {
    let cache = cache_env
        .iter()
        .filter(|(name, _)| !cfg.env.contains_key(name))
        .cloned();
    cfg.env
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .chain(cache)
        .collect()
}

/// Where the agent place leaves what stopped its egress proxy from starting.
const KEEP_ERR: &str = "/tmp/moor-egress-keep.err";
/// Time the agent place gets to install and start the egress proxy.
const KEEP_WITHIN: Duration = Duration::from_secs(120);

/// Starts or keeps the egress forwarder and tinyproxy of a machine's Box in its agent place.
fn keep_egress(
    cx: &Cx,
    handle: &BoxHandle,
    (egress_port, forward_port): (u16, u16),
    hosts: &[String],
) -> Result<(), String> {
    let script = format!(
        "( {} ) 2>{KEEP_ERR}",
        exec_env::keep_script(egress_port, forward_port, hosts)
    );
    // The script starts the proxy or keeps the one running, under a lock, so it may run twice.
    let spec =
        ExecSpec::new(Place::Agent, vec!["sh".to_owned(), "-c".to_owned(), script]).repeatable();
    let out = cx
        .driver()
        .exec(handle, &spec, Deadline(KEEP_WITHIN))
        .map_err(|e| format!("cannot start the egress proxy in the Box: {e}"))?;
    if out == (ExecOutcome::Exited { code: 0 }) {
        return Ok(());
    }
    let why = BoxPath::restore(KEEP_ERR)
        .ok()
        .and_then(|path| {
            cx.driver()
                .peek_file(handle, Place::Agent, &path)
                .ok()
                .flatten()
        })
        .and_then(|text| text.lines().last().map(str::to_owned))
        .filter(|why| !why.is_empty())
        .or_else(|| crate::ops::common::setup::failed::container(cx, handle))
        .unwrap_or_default();
    Err(format!(
        "the egress proxy could not start in the Box's agent place: {why}"
    ))
}

/// The user of Box `box_id` and the rules its Proxy would run.
fn box_plan(
    cx: &Cx,
    box_id: BoxId,
    repo: Option<&ForgeRepo>,
    extra: &BTreeMap<String, Vec<String>>,
) -> Result<(UserId, Option<String>, ProxyPlan), String> {
    let Requester {
        user,
        branch,
        others,
    } = requester(cx, box_id)?;
    let mcp = crate::ops::users::mcp::for_box(cx, Some(user)).map_err(|e| e.error.to_string())?;
    // A Box with a profile fetches its profile checkout even when it has no repo.
    let has_profile = cx
        .store()
        .map_err(|e| e.error.to_string())?
        .box_profile(box_id)
        .map_err(|e| e.to_string())?
        .is_some();
    let repos: Vec<ForgeRepo> = repo.into_iter().cloned().chain(others).collect();
    let plan = plan(cx, &repos, &mcp.servers, has_profile, extra)?;
    Ok((user, branch, plan))
}

/// What a Box's row says its Proxy acts for.
struct Requester {
    /// The user the Box acts for.
    user: UserId,
    /// The branch moor ties the Box to, which its pushes may move.
    branch: Option<String>,
    /// The repos the Box lists after its primary.
    others: Vec<ForgeRepo>,
}

/// The user the Box acts for, its branch and the repos it lists after its primary, read from its
/// row. A claimed place has no primary, so it lists its claim's repos, the task's first.
fn requester(cx: &Cx, box_id: BoxId) -> Result<Requester, String> {
    let row = cx
        .store()
        .map_err(|e| e.error.to_string())?
        .box_row(box_id)
        .map_err(|e| e.to_string())?;
    let Some(BoxRow {
        user: Some(user),
        repo,
        kind,
        branch,
        ..
    }) = row
    else {
        return Err(format!("Box {} acts for no user", box_id.get()));
    };
    let others = match (repo, kind) {
        (Some(r), _) => r.others.into_iter().map(|l| l.repo).collect(),
        (None, BoxKind::Place(PlaceState::Claimed(claim))) => claim.repos().cloned().collect(),
        (None, _) => Vec::new(),
    };
    Ok(Requester {
        user,
        branch,
        others,
    })
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
    let cache_env = proxy.map(|p| p.cache_env.as_slice()).unwrap_or_default();
    let repo_env = under_repo_env(cache_env, cfg);
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
