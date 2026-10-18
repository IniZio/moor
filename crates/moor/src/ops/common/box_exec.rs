//! The Box exec environment of the ops: the Task's Proxy and the staged credentials for one exec.

use std::path::Path;
use std::time::Duration;

use moor_core::box_key::BoxKey;
use moor_core::config::proxy::{ResolvedRule, FORGE_CREDENTIAL};
use moor_core::config::TaskSettings;
use moor_core::domain::run::SyncMode;
use moor_core::exec_env::{self, BoxCtx, ExecEnv, ProxyCtx, Purpose, RUN_DEADLINE};
use moor_core::seam::agent_profile::ProxyAddr;
use moor_core::seam::driver::{ExecOutcome, ExecSpec};
use moor_core::seam::proc::Deadline;
use moor_core::seam::proxy_host::ProxySpec;
use moor_core::{BoxHandle, BoxId, BoxPath};

use crate::cx::Cx;
use crate::ops::common::proxy::{box_env, origin, plan};

/// Time allowed for removing the staged directory.
const CLEANUP_DEADLINE: Duration = Duration::from_secs(30);
/// Time allowed for staging the nonce and the CA.
const STAGE_DEADLINE: Duration = Duration::from_secs(60);

/// The Task's running Proxy, as a Box exec needs it.
pub(crate) struct BoxProxy {
    /// The Task the Proxy serves; names the staged directory.
    pub(crate) task: BoxId,
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
    /// The host worktree of the Task.
    pub(crate) worktree: &'a Path,
}

/// Starts or reuses the Task's Proxy for `cfg`'s egress.
///
/// # Errors
/// Returns a message when the plan cannot be made or the Proxy cannot start.
pub(crate) fn proxy(
    cx: &Cx,
    task: BoxId,
    handle: Option<BoxHandle>,
    worktree: &Path,
    cfg: &TaskSettings,
) -> Result<BoxProxy, String> {
    let origin = origin(cx, worktree);
    let plan = plan(cx, origin.as_deref())?;
    let repo_env: Vec<(String, String)> = cfg.env.clone().into_iter().collect();
    let spec = ProxySpec {
        task,
        box_handle: handle,
        host_cwd: worktree.to_path_buf(),
        task_repo: plan.repo,
        rules: plan.rules,
    };
    let rule_env = box_env(&spec.rules);
    let forge = forge_routed(&spec.rules);
    let ep = cx.proxy_host().ensure(&spec).map_err(|e| e.to_string())?;
    let addr = ProxyAddr {
        box_port: ep.box_port,
        nonce: ep.nonce,
        ca_pem: ep.ca_pem,
        box_env: exec_env::merged_env(repo_env, rule_env.clone()),
        forge,
    };
    Ok(BoxProxy {
        task,
        addr,
        origin,
        rule_env,
    })
}

/// Whether a rule sends GitHub through the Proxy with the Task's forge token.
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

/// A Box exec environment from `exec_env` plus the staged nonce directory its argv removes.
pub(crate) struct BoxExec {
    /// The environment the exec runs with.
    pub(crate) env: ExecEnv,
    /// The Box directory holding the staged nonce and CA, when there is a Proxy.
    staged: Option<BoxPath>,
}

/// Builds the environment for `purpose`; with a Proxy it first stages the nonce and CA.
///
/// The directory is under the Box user's home, outside the synced workdir, so a sync can neither
/// delete nor upload it. It is created exclusively (mode 0700) before anything is written, so a
/// path someone else planted is refused and left alone.
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
    let dir = BoxPath::under(
        box_home(cx, site)?.as_str(),
        &BoxKey::new(p.task).cred_dir(&format!("{:08x}{:08x}", cx.rng().u32(), cx.rng().u32())),
    )
    .map_err(|e| e.to_string())?;
    create(cx, site, &dir)?;
    stage(cx, site, &dir, &p.addr)?;
    let merged = exec_env::merged_env(repo_env, p.rule_env.clone());
    let ctx = BoxCtx {
        env: &merged,
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
    /// Removes the staged directory after an exec that never reached its prelude; best effort.
    ///
    /// Call it when the command exec returns an error or is interrupted. No-op without a Proxy.
    pub(crate) fn cleanup(&self, cx: &Cx, site: &Site<'_>) {
        if let Some(dir) = &self.staged {
            remove(cx, site, dir);
        }
    }

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

/// The Box user's home, read from the environment of a process on the Box.
fn box_home(cx: &Cx, site: &Site<'_>) -> Result<BoxPath, String> {
    let environ = BoxPath::restore("/proc/self/environ").map_err(|e| e.to_string())?;
    let text = cx
        .driver()
        .peek_file(site.handle, &environ, site.worktree)
        .map_err(|e| format!("could not find the Box home: {e}"))?
        .ok_or("the Box has no process environment")?;
    let home = text
        .split('\0')
        .find_map(|kv| kv.strip_prefix("HOME="))
        .ok_or("the Box user has no home directory")?;
    BoxPath::restore(home).map_err(|e| e.to_string())
}

/// Creates `dir` private and new; fails when the path exists or cannot be made.
fn create(cx: &Cx, site: &Site<'_>, dir: &BoxPath) -> Result<(), String> {
    let spec = ExecSpec {
        argv: vec![
            "mkdir".into(),
            "-m".into(),
            "700".into(),
            "--".into(),
            dir.as_str().to_owned(),
        ],
        env: Vec::new(),
        cwd: None,
        sync: SyncMode::None,
        output: None,
        host_cwd: Some(site.worktree.to_path_buf()),
        class: None,
    };
    match cx
        .driver()
        .exec(site.handle, &spec, Deadline(CLEANUP_DEADLINE))
    {
        Ok(ExecOutcome::Exited { code: 0 }) => Ok(()),
        _ => Err(format!(
            "{} already exists or could not be created on the Box",
            dir.as_str()
        )),
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
