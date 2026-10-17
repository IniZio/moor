//! The macOS Box exec under the agent's sandbox: the exec env reaches the command only through the profile.

use moor_core::boxscript::denials::{SETUP_REPORT_REL, SETUP_START_REL};
use moor_core::boxscript::quote;
use moor_core::config::TaskSettings;
use moor_core::exec_env::{self, ExecEnv, MacTask, Purpose};
use moor_core::seam::engine::{MacGrants, SandboxReq, Sandboxed};
use moor_core::{BoxPath, TaskName};

use super::{create, remove, strip_env, BoxExec, Site};
use crate::cx::Cx;

impl BoxExec {
    /// An exec that runs `env` as is.
    pub(super) const fn plain(env: ExecEnv, staged: Option<BoxPath>) -> Self {
        Self {
            env,
            staged,
            sandboxed: None,
        }
    }
}

/// Finishes `env` into a [`BoxExec`]; on a macOS Box it asks the engine for the sandbox.
///
/// A macOS Box always gets a credential directory: `staged` when the Proxy was staged, else a new
/// one. The engine writes the profile in it, and the exec's `EXIT` trap removes it.
///
/// # Errors
/// Returns the engine's message when it cannot build the sandbox; the directory is removed.
pub(super) fn seal(
    cx: &Cx,
    site: &Site<'_>,
    cfg: &TaskSettings,
    purpose: Purpose,
    mac: Option<MacTask<'_>>,
    exec: BoxExec,
) -> Result<BoxExec, String> {
    let (Some(mac), Some(workdir)) = (mac, site.workdir) else {
        return Ok(exec);
    };
    let BoxExec { env, staged, .. } = exec;
    let dir = if let Some(dir) = staged {
        dir
    } else {
        let dir = mac.layout.cred_dir(mac.nonce).map_err(|e| e.to_string())?;
        create(cx, site, &dir)?;
        dir
    };
    let report = matches!(purpose, Purpose::Setup { .. });
    let req = TaskName::parse(mac.key.as_str())
        .map(|task| SandboxReq {
            handle: site.handle.clone(),
            worktree: site.worktree.to_path_buf(),
            workdir: workdir.clone(),
            task,
            cred: dir.clone(),
            set_vars: env.vars.clone(),
            strip_env: strip_env(cx, site.provider),
            mac_grants: MacGrants {
                capabilities: cfg.repo_box.capabilities.clone(),
                sandbox: cfg.repo_box.sandbox.clone(),
            },
            report,
        })
        .map_err(|e| e.to_string());
    let got = match req {
        Ok(req) => cx.engine().sandbox(&req).map_err(|e| e.to_string()),
        Err(e) => Err(e),
    };
    match got {
        Ok(Some(mut s)) => {
            if report {
                s.lines.insert(0, report_line(workdir));
            }
            Ok(BoxExec {
                env,
                staged: Some(dir),
                sandboxed: Some(s),
            })
        }
        Ok(None) => Ok(BoxExec {
            env,
            staged: Some(dir),
            sandboxed: None,
        }),
        Err(e) => {
            remove(cx, site, &dir);
            Err(e)
        }
    }
}

/// `argv` started by the sandbox, after the prelude and the sandbox's own lines.
pub(super) fn argv(
    env: &ExecEnv,
    s: &Sandboxed,
    argv: Vec<String>,
    staged: Option<&BoxPath>,
) -> Vec<String> {
    let mut env = env.clone();
    env.prelude.extend(s.lines.iter().cloned());
    let mut full = s.prefix.clone();
    full.extend(argv);
    exec_env::wrap_argv(&env, full, staged)
}

/// Records the Box-local start time and the setup shell's pid (every setup process has a pid at
/// least that high), then sends nono's diagnostics, its only stderr output, to the setup report
/// file in the workdir.
fn report_line(workdir: &BoxPath) -> String {
    let (workdir, rel) = (workdir.as_str(), SETUP_REPORT_REL);
    let dir = rel.rsplit_once('/').map_or(".", |(d, _)| d);
    let start = quote(&format!("{workdir}/{SETUP_START_REL}"));
    format!(
        "mkdir -p {} && date '+%Y-%m-%d %H:%M:%S' > {start} && echo $$ >> {start} && exec 2>{}",
        quote(&format!("{workdir}/{dir}")),
        quote(&format!("{workdir}/{rel}")),
    )
}
