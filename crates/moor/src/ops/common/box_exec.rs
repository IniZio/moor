//! The Box exec environment of the ops: the Task's Proxy and the staged credentials for one exec.

use std::path::Path;
use std::time::Duration;

use moor_core::config::TaskSettings;
use moor_core::domain::run::SyncMode;
use moor_core::exec_env::{self, BoxCtx, ExecEnv, ProxyCtx, Purpose, RUN_DEADLINE};
use moor_core::seam::agent_profile::ProxyAddr;
use moor_core::seam::driver::ExecSpec;
use moor_core::seam::proc::Deadline;
use moor_core::seam::proxy_host::ProxySpec;
use moor_core::{BoxHandle, BoxPath, TaskId};

use crate::cx::Cx;
use crate::ops::common::provision::nonce;
use crate::ops::common::proxy::{box_env, origin, plan};

/// Time allowed for removing the staged directory.
const CLEANUP_DEADLINE: Duration = Duration::from_secs(30);
/// Time allowed for staging the nonce and the CA.
const STAGE_DEADLINE: Duration = Duration::from_secs(60);

/// The Task's running Proxy, as a Box exec needs it.
pub(crate) struct BoxProxy {
    /// The Proxy's address and credentials inside the Box.
    pub(crate) addr: ProxyAddr,
    /// The git origin URL of the worktree, when it has one.
    pub(crate) origin: Option<String>,
}

/// The Box an exec runs in and where it runs.
pub(crate) struct Site<'a> {
    /// The Box.
    pub(crate) handle: &'a BoxHandle,
    /// The Box's provider, which names the variables to strip.
    pub(crate) provider: &'a str,
    /// The directory commands run in; `None` keeps the driver's default.
    pub(crate) workdir: Option<&'a BoxPath>,
    /// The host worktree of the Task.
    pub(crate) worktree: &'a Path,
}

/// Starts or reuses the Task's Proxy for `cfg`'s egress.
///
/// # Errors
/// Returns a message when the plan cannot be made or the Proxy cannot start.
pub(crate) fn proxy(
    cx: &Cx,
    task: TaskId,
    handle: Option<BoxHandle>,
    worktree: &Path,
    cfg: &TaskSettings,
) -> Result<BoxProxy, String> {
    let origin = origin(cx, worktree);
    let plan = plan(cx, origin.as_deref(), cfg)?;
    let repo_env = plan.env.clone();
    let spec = ProxySpec {
        task,
        box_handle: handle,
        host_cwd: worktree.to_path_buf(),
        task_repo: plan.repo,
        rules: plan.rules,
    };
    let ep = cx.proxy_host().ensure(&spec).map_err(|e| e.to_string())?;
    let addr = ProxyAddr {
        box_port: ep.box_port,
        nonce: ep.nonce,
        ca_pem: ep.ca_pem,
        box_env: exec_env::merged_env(repo_env, box_env(&spec.rules)),
    };
    Ok(BoxProxy { addr, origin })
}

/// The variable names the `provider`'s platform puts in the Box that moor removes.
pub(crate) fn strip_env(cx: &Cx, provider: &str) -> Vec<String> {
    cx.config()
        .drivers
        .get(provider)
        .map_or_else(Vec::new, |d| d.strip_env.clone())
}

/// A Box exec environment from `exec_env` plus the staged nonce directory its argv removes.
pub(crate) struct BoxExec {
    /// The environment the exec runs with.
    pub(crate) env: ExecEnv,
    /// The Box directory holding the staged nonce and CA, when there is a Proxy.
    staged: Option<BoxPath>,
}

/// Builds the environment for `purpose`; with a Proxy it first stages the nonce and CA.
///
/// The directory is under `/tmp`, outside the synced workdir, so a sync can neither delete nor
/// upload it; the driver creates it private (mode 0700).
///
/// # Errors
/// Returns a message when staging fails or the environment cannot be built.
pub(crate) fn prepare(
    cx: &Cx,
    site: &Site<'_>,
    proxy: Option<&BoxProxy>,
    cfg: &TaskSettings,
    purpose: Purpose,
) -> Result<BoxExec, String> {
    let strip = strip_env(cx, site.provider);
    let repo_env: Vec<(String, String)> = cfg.env.clone().into_iter().collect();
    let Some(p) = proxy else {
        let ctx = BoxCtx {
            env: &repo_env,
            proxy: None,
            origin: None,
            workdir: site.workdir,
            strip: &strip,
        };
        let env = exec_env::build(&ctx, purpose)?;
        return Ok(BoxExec { env, staged: None });
    };
    let dir = BoxPath::restore(&format!("/tmp/moor-exec-{}", nonce(cx.rng())))
        .map_err(|e| e.to_string())?;
    stage(cx, site, &dir, &p.addr)?;
    let ctx = BoxCtx {
        env: &p.addr.box_env,
        proxy: Some(ProxyCtx {
            box_port: p.addr.box_port,
            cred: &dir,
        }),
        origin: p.origin.as_deref(),
        workdir: site.workdir,
        strip: &strip,
    };
    match exec_env::build(&ctx, purpose) {
        Ok(env) => Ok(BoxExec {
            env,
            staged: Some(dir),
        }),
        Err(e) => {
            remove(cx, site, &dir);
            Err(e)
        }
    }
}

/// Writes the Proxy nonce and CA into `dir` in the Box; a failure removes `dir` and names the step.
fn stage(cx: &Cx, site: &Site<'_>, dir: &BoxPath, addr: &ProxyAddr) -> Result<(), String> {
    let staged = exec_env::stage_steps(dir, &addr.nonce, &addr.ca_pem)?;
    let (steps, texts): (Vec<_>, Vec<_>) = staged.into_iter().unzip();
    let driver = cx.driver();
    let failed = |text: &str, why: &dyn ToString| {
        remove(cx, site, dir);
        format!("{text}: {}", why.to_string())
    };
    let connect = driver
        .connect_argv(site.handle, site.worktree)
        .map_err(|e| failed("could not reach the Box", &e))?;
    let deadline = Deadline(STAGE_DEADLINE);
    let results = driver.batch(site.handle, &connect, &steps, site.worktree, deadline);
    for (i, text) in texts.iter().enumerate() {
        match results.get(i) {
            Some(Ok(_)) => {}
            Some(Err(e)) => return Err(failed(text, e)),
            None => return Err(failed(text, &"the step did not run")),
        }
    }
    Ok(())
}

impl BoxExec {
    /// `argv` wrapped so the prelude runs first and the staged directory is removed on exit.
    pub(crate) fn argv(&self, argv: Vec<String>) -> Vec<String> {
        exec_env::wrap_argv(&self.env, argv, self.staged.as_ref())
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

/// Removes the Box directory `dir`; best effort, errors are ignored.
fn remove(cx: &Cx, site: &Site<'_>, dir: &BoxPath) {
    let spec = ExecSpec {
        argv: vec!["rm".into(), "-rf".into(), dir.as_str().to_owned()],
        env: Vec::new(),
        cwd: None,
        sync: SyncMode::None,
        output: None,
        host_cwd: Some(site.worktree.to_path_buf()),
        class: None,
    };
    let _ = cx
        .driver()
        .exec(site.handle, &spec, Deadline(CLEANUP_DEADLINE));
}

#[cfg(test)]
mod tests;
