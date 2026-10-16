//! Runs the repo `teardown:` script in the Box before it is released.

use std::path::Path;

use moor_core::boxscript::teardown::teardown_script;
use moor_core::config::Flags;
use moor_core::domain::boxes::BoxState;
use moor_core::domain::run::SyncMode;
use moor_core::exec_env::Purpose;
use moor_core::seam::driver::{ExecOutcome, ExecSpec};
use moor_core::Warning;

use super::held;
use crate::cx::Cx;
use crate::ops::common::box_exec::{self, Site};
use crate::ops::common::facts::resolved;
use crate::ops::common::provision::{moor_yaml::for_teardown, recorded_workdir};
use crate::store::TaskRow;

/// Runs the repo `teardown:` lines as one synced script in a ready Box.
///
/// Nothing runs without lines or a live Box; a failure only warns. A worktree `.moor.yaml` that
/// differs from the base commit is not used unless `trusted`. The env is the agent's Box env.
pub(super) fn teardown(cx: &Cx, row: &TaskRow, trusted: bool) {
    let ready = row.boxed.as_ref().filter(|b| b.state == BoxState::Ready);
    let Some((b, handle)) = ready.and_then(|b| Some((b, held(b)?))) else {
        return;
    };
    let repo = match for_teardown(cx, row, trusted) {
        Ok(repo) => repo,
        Err(e) => return warn_teardown(cx, &e.error.to_string()),
    };
    let cfg = resolved(cx, repo.as_ref(), &Flags::default());
    if cfg.teardown.is_empty() {
        return;
    }
    let worktree = Path::new(row.worktree.as_deref().unwrap_or(&row.repo));
    let workdir = b.workdir.as_deref().and_then(recorded_workdir);
    let proxy = box_exec::proxy(cx, row.id, Some(handle.clone()), worktree, &cfg);
    let site = Site {
        handle,
        provider: b.provider.as_str(),
        workdir: workdir.as_ref(),
        worktree,
    };
    let exec = match box_exec::prepare(cx, &site, proxy.as_ref().ok(), &cfg, Purpose::Teardown) {
        Ok(exec) => exec,
        Err(e) => return warn_teardown(cx, &e),
    };
    let script = ["sh", "-c", &teardown_script(&cfg.teardown)].map(str::to_owned);
    let spec = ExecSpec {
        argv: exec.argv(script.to_vec()),
        env: exec.vars(),
        cwd: exec.cwd(),
        sync: SyncMode::Full,
        output: None,
        host_cwd: row.worktree.as_ref().map(Into::into),
        class: Some(cfg.size.value.as_str().to_owned()),
    };
    let reason = match cx.driver().exec(handle, &spec, exec.deadline()) {
        Ok(ExecOutcome::Exited { code: 0 }) => return,
        Ok(ExecOutcome::Exited { code }) => format!("exit {code}"),
        Ok(ExecOutcome::Interrupted) => "interrupted".to_owned(),
        Err(e) => e.to_string(),
    };
    warn_teardown(cx, &reason);
}

/// Warns that teardown did not finish; the Box is released regardless.
fn warn_teardown(cx: &Cx, reason: &str) {
    cx.warn(Warning {
        code: "teardown_failed".to_owned(),
        message: format!("teardown did not finish: {reason}; the Box is released anyway"),
    });
}
