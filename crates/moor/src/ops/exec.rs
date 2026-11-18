//! `exec`: run a command in an open Box with its stdio streamed byte for byte, no terminal.
//!
//! Unlike `run` it keeps no run record. The Box's turn is held only to wake an asleep Box's
//! machine; the command then counts as a run on the Box until its `exec_ran` event is recorded,
//! so a command can run beside another run, a steer still reaches the agent, and no close can
//! come between the command and its record.
//!
//! A command only looks into the Box, so an asleep Box stays Asleep: its agent is not started
//! again and nothing queued for it is sent until a steer or a wake. Its machine sleeps again
//! once nothing runs on it.

use std::time::Duration;

use moor_core::boxscript::{command_line, shell_argv};
use moor_core::config::Flags;
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::exec_env::Purpose;
use moor_core::seam::driver::{ExecOutcome, ExecSpec, Place};
use moor_core::seam::proc::Pipes;
use moor_core::view::ExecView;
use moor_core::{BoxHandle, ErrorKind, ExecReq};

use crate::cx::owners::Cmd;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::box_exec::{self, Site};
use crate::ops::common::facts::settings_of;
use crate::ops::common::provisioning::dead_provisioning;
use crate::ops::common::target::{self, Rule};
use crate::ops::common::wake::{rest_machine, wake_to_look};
use crate::ops::run::ready;
use crate::store::BoxRow;

/// Runs `req` in its Box, waking its machine first, with its stdio on `pipes`.
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
    let first = open()?;
    dead_provisioning(cx, &first)?;
    let turn = cx.turn(first.id, Cmd::Exec)?;
    let mut row = open()?;
    if wake_to_look(cx, store, &row)? {
        row = open()?;
    }
    let running = turn.begin_run();
    drop(turn);
    let ran = exec_in(cx, req, &row, pipes);
    drop(running);
    if row.lifecycle == BoxLifecycle::Asleep {
        rest_machine(cx, store, row.id);
    }
    ran
}

/// Runs `req` in the work place of `row`, whose machine is up, and records it.
fn exec_in(
    cx: &Cx,
    req: &ExecReq,
    row: &BoxRow,
    pipes: Pipes<'_>,
) -> Result<ExecView, CommandError> {
    let failed = |m: String| CommandError::new(ErrorKind::Failed, m);
    let store = cx.store()?;
    let target = ready::target(store, row)?;
    let cfg = settings_of(cx, row, &Flags::default())?;
    let proxy = box_exec::proxy(
        cx,
        row.id,
        Some((target.handle.clone(), row.hands_inc())),
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
        provider: row.hands.as_ref().map_or("", |b| b.provider.as_str()),
        workdir: target.workdir.as_ref(),
    };
    let env = box_exec::prepare(cx, &site, Some(&proxy), &cfg, Purpose::Run).map_err(failed)?;
    let script = match req.argv.as_slice() {
        [line] => line.clone(),
        argv => command_line(argv),
    };
    let spec = ExecSpec::new(Place::Work, env.argv(shell_argv(&script)))
        .env(env.vars())
        .cwd(env.cwd());
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
        // The driver's own error stays the source, so a caller can tell a cut link from the rest.
        Err(e) => Err(CommandError::new(ErrorKind::Failed, e)),
    }
}

#[cfg(test)]
mod tests;
