//! Installs the agent in a Box that lacks it.

use std::path::Path;
use std::time::Duration;

use moor_core::config::BoxSettings;
use moor_core::exec_env::Purpose;
use moor_core::seam::driver::{ExecOutcome, ExecSpec};
use moor_core::seam::proc::{Deadline, Output};
use moor_core::{BoxHandle, BoxId, Warning};

use crate::cx::Cx;
use crate::ops::common::box_exec::{self, BoxProxy, Site};
use crate::ops::common::dotfiles::{private_log, tail};

/// Wall-clock allowance for the install inside the Box.
pub(crate) const INSTALL_DEADLINE: Duration = Duration::from_mins(10);

/// The warning for an install that did not succeed.
pub(crate) fn warning(reason: &str) -> Warning {
    Warning {
        code: "agent_install_failed".to_owned(),
        message: format!("installing the agent in the Box failed: {reason}"),
    }
}

/// Runs the agent's install script in the Box with the env and Proxy setup gets, logging to
/// `log`. The error is why it could not run to an exit.
fn run(
    cx: &Cx,
    site: &Site<'_>,
    cfg: &BoxSettings,
    proxy: Option<&BoxProxy>,
    script: String,
    log: &Path,
) -> Result<ExecOutcome, String> {
    let env = box_exec::prepare(cx, site, proxy, cfg, Purpose::Run)
        .map_err(|e| format!("could not prepare the install: {e}"))?;
    let exec = ExecSpec {
        argv: env.argv(vec!["sh".to_owned(), "-c".to_owned(), script]),
        env: env.vars(),
        cwd: None,
        output: Some(Output::Log(log.to_path_buf())),
    };
    cx.driver()
        .exec(site.handle, &exec, Deadline(INSTALL_DEADLINE))
        .map_err(|e| e.to_string())
}

/// Installs the agent in the Box when the engine knows how and the config allows it; the script
/// itself does nothing when the agent command is already there. Does nothing when the Proxy did
/// not start: the launch fails on that first.
///
/// # Errors
/// Why the install did not succeed, then the last output lines when there was output.
pub(crate) fn apply(
    cx: &Cx,
    (handle, provider): (&BoxHandle, &str),
    id: BoxId,
    cfg: &BoxSettings,
    proxy: &Result<Option<BoxProxy>, String>,
) -> Result<(), String> {
    let (true, Some(install), Ok(proxy)) = (cfg.install_agent, cx.engine().agent_install(), proxy)
    else {
        return Ok(());
    };
    let log = cx
        .state_dir()
        .map_err(|e| e.error.to_string())?
        .join("agent-install")
        .join(format!("{}.log", id.get()));
    private_log(&log).map_err(|e| format!("could not create the install log: {e}"))?;
    let site = Site {
        handle,
        provider,
        workdir: None,
    };
    let reason = match run(cx, &site, cfg, proxy.as_ref(), install.script, &log) {
        Ok(ExecOutcome::Exited { code: 0 }) => return Ok(()),
        Ok(ExecOutcome::Exited { code }) => format!("exit {code}"),
        Ok(ExecOutcome::Interrupted) => "interrupted".to_owned(),
        Err(e) => e,
    };
    let logged = std::fs::read_to_string(&log)
        .map(|s| tail(&s))
        .unwrap_or_default();
    Err(if logged.is_empty() {
        reason
    } else {
        format!("{reason}\n{logged}")
    })
}
