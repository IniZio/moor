//! Runs the repo `teardown:` script in the Box before it is released.

use std::path::Path;

use moor_core::boxscript::teardown::teardown_script;
use moor_core::config::{BoxSettings, Flags};
use moor_core::domain::boxes::BoxState;
use moor_core::exec_env::Purpose;
use moor_core::seam::driver::{ExecOutcome, ExecSpec};
use moor_core::{BoxHandle, ErrorKind, Warning};

use super::held;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::box_exec::{self, BoxProxy, Site};
use crate::ops::common::facts::resolved;
use crate::ops::common::provision::{moor_yaml::for_teardown, recorded_workdir};
use crate::store::BoxRow;

/// Runs the repo `teardown:` lines as one script in a ready Box.
///
/// Nothing runs without lines or a live Box; a failure only warns, except a Proxy that will not
/// start, which fails the close unless `force` is set, when it warns like the rest. A worktree `.moor.yaml` that
/// differs from the base commit is not used unless `trusted`. The env is the agent's Box env.
pub(super) fn teardown(
    cx: &Cx,
    row: &BoxRow,
    trusted: bool,
    force: bool,
) -> Result<(), CommandError> {
    let ready = row.boxed.as_ref().filter(|b| b.state == BoxState::Ready);
    let Some((b, handle)) = ready.and_then(|b| Some((b, held(b)?))) else {
        return Ok(());
    };
    let repo = match for_teardown(cx, row, trusted) {
        Ok(repo) => repo,
        Err(e) => {
            warn_teardown(cx, &e.error.to_string());
            return Ok(());
        }
    };
    let cfg = resolved(cx, repo.as_ref(), &Flags::default());
    if cfg.teardown.is_empty() {
        return Ok(());
    }
    let worktree = Path::new(row.worktree.as_deref().unwrap_or(&row.repo));
    let workdir = b.workdir.as_deref().and_then(recorded_workdir);
    let Some(proxy) = teardown_proxy(cx, row, handle, worktree, &cfg, force)? else {
        return Ok(());
    };
    let site = Site {
        handle,
        provider: b.provider.as_str(),
        workdir: workdir.as_ref(),
    };
    let exec = match box_exec::prepare(cx, &site, Some(&proxy), &cfg, Purpose::Teardown) {
        Ok(exec) => exec,
        Err(e) => {
            warn_teardown(cx, &e);
            return Ok(());
        }
    };
    let script = ["sh", "-c", &teardown_script(&cfg.teardown)].map(str::to_owned);
    let spec = ExecSpec {
        argv: exec.argv(script.to_vec()),
        env: exec.vars(),
        cwd: exec.cwd(),
        output: None,
        class: Some(cfg.size.value.as_str().to_owned()),
    };
    let result = cx.driver().exec(handle, &spec, exec.deadline());
    let reason = match result {
        Ok(ExecOutcome::Exited { code: 0 }) => return Ok(()),
        Ok(ExecOutcome::Exited { code }) => format!("exit {code}"),
        Ok(ExecOutcome::Interrupted) => "interrupted".to_owned(),
        Err(e) => e.to_string(),
    };
    warn_teardown(cx, &reason);
    Ok(())
}

/// The Proxy teardown runs through; none when it will not start and `force` lets the close go on.
fn teardown_proxy(
    cx: &Cx,
    row: &BoxRow,
    handle: &BoxHandle,
    worktree: &Path,
    cfg: &BoxSettings,
    force: bool,
) -> Result<Option<BoxProxy>, CommandError> {
    match box_exec::proxy(cx, row.id, Some(handle.clone()), worktree, cfg) {
        Ok(proxy) => Ok(Some(proxy)),
        Err(e) if force => {
            warn_teardown(cx, &format!("the Proxy could not start: {e}"));
            Ok(None)
        }
        Err(e) => Err(CommandError::new(
            ErrorKind::Failed,
            format!(
                "the Proxy could not start: {e}; retry the close once it starts, or pass \
                 --force to close without teardown"
            ),
        )),
    }
}

/// Warns that teardown did not finish; the Box is released regardless.
fn warn_teardown(cx: &Cx, reason: &str) {
    cx.warn(Warning {
        code: "teardown_failed".to_owned(),
        message: format!("teardown did not finish: {reason}; the Box is released anyway"),
    });
}
