//! `exec`: run a command in an open Box with its stdio streamed byte for byte, no terminal.
//!
//! Unlike `run` it keeps no run record and takes no Box lock, so it never waits on one. Each
//! command that ran leaves one `exec_ran` event, so the Box's owner can see what ran in it.

use std::time::Duration;

use moor_core::boxscript::{command_line, shell_argv};
use moor_core::config::Flags;
use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::exec_env::Purpose;
use moor_core::seam::driver::{ExecOutcome, ExecSpec};
use moor_core::seam::proc::Pipes;
use moor_core::view::ExecView;
use moor_core::{BoxHandle, ErrorKind, ExecReq};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::box_exec::{self, Site};
use crate::ops::common::facts::settings_of;
use crate::ops::common::provisioning::dead_provisioning;
use crate::ops::common::target::{self, Rule};
use crate::ops::common::wake::wake_and_revive;
use crate::ops::run::ready;

/// Runs `req` in its Box, waking it first, with its stdio on `pipes`.
///
/// # Errors
/// [`CommandError`] when the command could not start: no open Box, no usable machine, a Proxy
/// that will not start, or a provider that cannot stream a command's stdio.
pub(crate) fn run(cx: &Cx, req: &ExecReq, pipes: Pipes<'_>) -> Result<ExecView, CommandError> {
    let failed = |m: String| CommandError::new(ErrorKind::Failed, m);
    if req.argv.is_empty() {
        return Err(CommandError::new(ErrorKind::Usage, "no command to run"));
    }
    let store = cx.store()?;
    let open = || -> Result<_, CommandError> {
        target::resolve(store, &req.target, Rule::Open)?
            .ok_or_else(|| failed(format!("no open Box {}", req.target)))
    };
    let mut row = open()?;
    dead_provisioning(cx, &row)?;
    if wake_and_revive(cx, store, &row)? {
        row = open()?;
    }
    let target = ready::target(store, &row)?;
    let cfg = settings_of(cx, &row, &Flags::default())?;
    let proxy = box_exec::proxy(
        cx,
        row.id,
        Some(target.handle.clone()),
        row.repo.as_ref().map(|r| &r.repo),
        &cfg,
    )
    .map_err(|e| {
        failed(format!(
            "the Proxy could not start: {e}; retry once it starts"
        ))
    })?;
    let site = Site {
        handle: &target.handle,
        provider: row.boxed.as_ref().map_or("", |b| b.provider.as_str()),
        workdir: target.workdir.as_ref(),
    };
    let env = box_exec::prepare(cx, &site, Some(&proxy), &cfg, Purpose::Run).map_err(failed)?;
    let script = match req.argv.as_slice() {
        [line] => line.clone(),
        argv => command_line(argv),
    };
    let spec = ExecSpec {
        argv: env.argv(shell_argv(&script)),
        env: env.vars(),
        cwd: env.cwd(),
        output: None,
    };
    let view = within(cx, &target.handle, &spec, pipes, req.timeout_secs)?;
    let event = ExtraEvent::ExecRan {
        command: script,
        exit: view.exit,
        timed_out: view.timed_out,
    };
    store.apply(Decision::new(row.id).event(event))?;
    Ok(view)
}

/// Runs `spec` on `handle` with its stdio on `pipes`, killing it once it has run
/// `timeout_secs`; how it ended.
fn within(
    cx: &Cx,
    handle: &BoxHandle,
    spec: &ExecSpec,
    pipes: Pipes<'_>,
    timeout_secs: Option<u32>,
) -> Result<ExecView, CommandError> {
    let clock = cx.clock();
    let end = timeout_secs.map(|secs| clock.instant() + Duration::from_secs(secs.into()));
    let late = || end.is_some_and(|end| clock.instant() >= end);
    let Pipes {
        stdin,
        stdout,
        stderr,
        stop: gone,
    } = pipes;
    let stop = || gone() || late();
    let pipes = Pipes {
        stdin,
        stdout,
        stderr,
        stop: &stop,
    };
    match cx.driver().exec_piped(handle, spec, pipes) {
        Ok(ExecOutcome::Exited { code }) => Ok(ExecView {
            exit: Some(code),
            timed_out: false,
        }),
        Ok(ExecOutcome::Interrupted) => Ok(ExecView {
            exit: None,
            timed_out: late(),
        }),
        Err(e) => Err(CommandError::new(ErrorKind::Failed, e.to_string())),
    }
}

#[cfg(test)]
mod tests;
