//! `exec`: run a command in an open Box with its stdio streamed byte for byte, no terminal.
//!
//! Unlike `run` it keeps no run record and takes no Box lock, so it never waits on one.

use moor_core::boxscript::{command_line, shell_argv};
use moor_core::config::Flags;
use moor_core::exec_env::Purpose;
use moor_core::seam::driver::{ExecOutcome, ExecSpec};
use moor_core::seam::proc::Pipes;
use moor_core::view::ExecView;
use moor_core::{ErrorKind, ExecReq};

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
    match cx.driver().exec_piped(&target.handle, &spec, pipes) {
        Ok(ExecOutcome::Exited { code }) => Ok(ExecView { exit: Some(code) }),
        Ok(ExecOutcome::Interrupted) => Ok(ExecView { exit: None }),
        Err(e) => Err(failed(e.to_string())),
    }
}

#[cfg(test)]
mod tests;
