//! Installs the agent in a Box that lacks it.

use std::path::Path;
use std::time::Duration;

use moor_core::boxscript::one_at_a_time;
use moor_core::config::BoxSettings;
use moor_core::domain::event::PostCloneStep;
use moor_core::exec_env::Purpose;
use moor_core::seam::driver::{ExecOutcome, ExecSpec, Place};
use moor_core::seam::proc::{Deadline, Output};
use moor_core::{BoxHandle, BoxId, Warning};

use crate::cx::Cx;
use crate::ops::common::box_exec::{self, BoxProxy, Site};
use crate::ops::common::dotfiles::{private_log, tail};
use crate::ops::common::step::{exec_failure, timed_result};
use crate::store::Store;

/// Wall-clock allowance for the install inside the Box.
pub(crate) const INSTALL_DEADLINE: Duration = Duration::from_mins(10);

/// Wall-clock allowance for asking whether the agent is there.
const PRESENT_DEADLINE: Duration = Duration::from_mins(1);

/// Whether a script may run again when a lost machine link leaves unknown whether it ran.
#[derive(Clone, Copy)]
pub(crate) enum Repeat {
    /// Runs at most once.
    Once,
    /// Safe to run twice, so it is asked again across a lost link.
    Safe,
}

/// The warning for an install that did not succeed.
pub(crate) fn warning(reason: &str) -> Warning {
    Warning {
        code: "agent_install_failed".to_owned(),
        message: format!("installing the agent in the Box failed: {reason}"),
    }
}

/// Runs an install script in `place` in the Box with the env and Proxy setup gets, logging to
/// `log`, for at most `deadline`, asked again across a lost machine link when `repeat` allows. The
/// error is why it could not run to an exit; `what` names the step in it when the deadline hit.
#[allow(
    clippy::too_many_arguments,
    reason = "the Box, place, settings, Proxy, script, log and deadline"
)]
pub(crate) fn run(
    cx: &Cx,
    site: &Site<'_>,
    place: Place,
    cfg: &BoxSettings,
    proxy: Option<&BoxProxy>,
    (script, repeat): (String, Repeat),
    log: &Path,
    (what, deadline): (&str, Duration),
) -> Result<ExecOutcome, String> {
    let env = box_exec::prepare(cx, site, proxy, cfg, Purpose::Run)
        .map_err(|e| format!("could not prepare the install: {e}"))?;
    let exec = ExecSpec::new(
        place,
        env.argv(vec!["sh".to_owned(), "-c".to_owned(), script]),
    )
    .env(env.vars())
    .output(Some(Output::Log(log.to_path_buf())));
    let exec = match repeat {
        Repeat::Once => exec,
        Repeat::Safe => exec.repeatable(),
    };
    cx.driver()
        .exec(site.handle, &exec, Deadline(deadline))
        .map_err(|e| exec_failure(what, &e))
}

/// Installs the agent in the Box when the engine knows how and the config allows it; the script
/// itself does nothing when the agent command is already there. Does nothing when the Proxy did
/// not start: the launch fails on that first.
///
/// # Errors
/// Why the install did not succeed, then the last output lines when there was output.
pub(crate) fn apply(
    cx: &Cx,
    store: &Store,
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
    timed_result(cx, store, id, PostCloneStep::AgentInstall, || {
        install_in_box(cx, &site, cfg, proxy.as_ref(), &install.script, &log)
    })
}

/// Whether the Box can start its agent as it is, or moor has no way to tell, which a fork that
/// holds an install asks since its line reruns and its caches may have lost it. A failed check counts as no.
pub(crate) fn present(
    cx: &Cx,
    (handle, provider): (&BoxHandle, &str),
    id: BoxId,
    cfg: &BoxSettings,
    proxy: &Result<Option<BoxProxy>, String>,
) -> bool {
    let (true, Some(install), Ok(proxy)) = (cfg.install_agent, cx.engine().agent_install(), proxy)
    else {
        return true;
    };
    let Ok(dir) = cx.state_dir() else {
        return false;
    };
    let log = dir.join("agent-install").join(format!("{}.log", id.get()));
    if private_log(&log).is_err() {
        return false;
    }
    let site = Site {
        handle,
        provider,
        workdir: None,
    };
    let limit = ("agent check", PRESENT_DEADLINE);
    let ran = run(
        cx,
        &site,
        Place::Agent,
        cfg,
        proxy.as_ref(),
        (install.present, Repeat::Safe),
        &log,
        limit,
    );
    matches!(ran, Ok(ExecOutcome::Exited { code: 0 }))
}

/// Runs the install script in the Box, one copy at a time and asked again across a lost machine
/// link, since it does nothing once the agent is there; why it did not succeed, with the log tail.
fn install_in_box(
    cx: &Cx,
    site: &Site<'_>,
    cfg: &BoxSettings,
    proxy: Option<&BoxProxy>,
    script: &str,
    log: &Path,
) -> Result<(), String> {
    let reason = match run(
        cx,
        site,
        Place::Agent,
        cfg,
        proxy,
        (one_at_a_time(script), Repeat::Safe),
        log,
        ("agent install", INSTALL_DEADLINE),
    ) {
        Ok(ExecOutcome::Exited { code: 0 }) => return Ok(()),
        Ok(ExecOutcome::Exited { code }) => format!("exit {code}"),
        Ok(ExecOutcome::Interrupted) => "interrupted".to_owned(),
        Err(e) => e,
    };
    let logged = std::fs::read_to_string(log)
        .map(|s| tail(&s))
        .unwrap_or_default();
    Err(if logged.is_empty() {
        reason
    } else {
        format!("{reason}\n{logged}")
    })
}
