//! `run`: execute a command on the Task's Box, keep a run record, report how it ended.
//!
//! The Box lock is held from the run row's creation to its completion.

use std::path::{Path, PathBuf};

use moor_core::boxscript::command_line;
use moor_core::config::{Flags, TaskSettings};
use moor_core::domain::boxes::BoxState;
use moor_core::domain::decision::Decision;
use moor_core::domain::run::SyncMode;
use moor_core::domain::task::{TaskInput, TaskState};
use moor_core::exec_env::Purpose;
use moor_core::seam::driver::{DriverError, ExecOutcome, ExecSpec};
use moor_core::seam::proc::Output;
use moor_core::view::run::RunValue;
use moor_core::{ErrorKind, RunId, RunReq, Sha, Warning};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::box_exec::{self, BoxExec, Site};
use crate::ops::common::boxlock::{self, BoxLock};
use crate::ops::common::facts::resolved;
use crate::ops::common::provision::moor_yaml;
use crate::ops::common::provisioning::dead_provisioning;
use crate::ops::common::wake::wake_and_revive;
use crate::store::{NewRun, RunOutcome, Store, StoreError, TaskRow};

mod ready;
mod record;
mod sync;

/// Run `run`.
///
/// # Errors
/// [`CommandError`] when the command could not start: unknown Task, no usable Box, a busy Box,
/// a `.moor.yaml` that differs from the base commit without trust, or a failure of the driver or the store before the command ran.
pub(crate) fn run(cx: &Cx, req: &RunReq) -> Result<RunValue, CommandError> {
    if req.argv.is_empty() {
        return Err(CommandError::new(ErrorKind::Usage, "no command to run"));
    }
    record::usage(req)?;
    sync::usage(req)?;
    let store = cx.store()?;
    let name = req.task.as_str();
    let found = store
        .latest_task_by_name(&req.task)?
        .ok_or_else(|| CommandError::new(ErrorKind::Failed, format!("no task {name}")))?;
    dead_provisioning(cx, &found)?;
    let _held = BoxLock::acquire(cx.clock(), cx.state_dir()?, found.id, "run", boxlock::WAIT)?;
    let row = store
        .task(found.id)?
        .ok_or_else(|| CommandError::new(ErrorKind::Failed, format!("no task {name}")))?;
    locked(cx, store, req, &row)
}

/// The part of `run` that needs the Box lock, on the row read after taking it.
fn locked(cx: &Cx, store: &Store, req: &RunReq, row: &TaskRow) -> Result<RunValue, CommandError> {
    dead_provisioning(cx, row)?;
    let gone = row
        .boxed
        .as_ref()
        .is_some_and(|b| matches!(b.state, BoxState::Gone | BoxState::Deleted));
    let woken;
    let row = if !gone && wake_and_revive(cx, store, row)? {
        woken = store.task(row.id)?.ok_or_else(|| {
            CommandError::new(ErrorKind::Failed, format!("no task {}", row.name.as_str()))
        })?;
        &woken
    } else {
        row
    };
    let target = ready::target(store, row)?;
    launch(cx, store, req, row, &target)
}

/// Records the run, executes the command in the Box and records how it ended.
fn launch(
    cx: &Cx,
    store: &Store,
    req: &RunReq,
    row: &TaskRow,
    target: &ready::Target,
) -> Result<RunValue, CommandError> {
    let repo = moor_yaml::guard(cx, row, req.trust_moor_yaml)?;
    let cfg = resolved(cx, repo.as_ref(), &Flags::default());
    let sync = sync::choose(cx, store, req, row)?;
    let (sha, argv) = plan(cx, req, &target.worktree, &cfg.setup)?;
    let id = store.begin_run(NewRun {
        task: row.id,
        argv: req.argv.clone(),
        record: req.record.clone(),
        sha: sha.clone(),
        sync: sync_name(sync).to_owned(),
        pid: std::process::id(),
    })?;
    if let Err(e) = mark_working(store, row) {
        finish(store, id, None);
        return Err(e);
    }
    let (output, log) = match output(cx, req, id) {
        Ok(got) => got,
        Err(e) => {
            finish(store, id, None);
            return Err(e);
        }
    };
    let exec = box_env(cx, row, target, &cfg)
        .inspect_err(|e| finish(store, id, Some(e.error.to_string())))?;
    let spec = ExecSpec {
        argv: exec.argv(argv),
        env: exec.vars(),
        cwd: exec.cwd(),
        sync,
        output: Some(output),
        host_cwd: Some(target.worktree.clone()),
        class: Some(cfg.size.value.as_str().to_owned()),
    };
    let outcome = match exec_in_box(cx, row, target, &exec, &spec) {
        Ok(outcome) => outcome,
        Err(e) => {
            let message = e.to_string();
            finish(store, id, Some(message.clone()));
            return Err(CommandError::new(ErrorKind::Failed, message));
        }
    };
    let (exit, interrupted) = match outcome {
        ExecOutcome::Exited { code } => (Some(code), false),
        ExecOutcome::Interrupted => (None, true),
    };
    store.finish_run(id, RunOutcome::ended(exit, interrupted))?;
    Ok(RunValue {
        ran: true,
        run: Some(id),
        exit,
        interrupted,
        sha: sha.map(|s| s.as_str().to_owned()),
        sync,
        record: req.record.clone(),
        log,
    })
}

/// Runs the command; when it may never have started, drops the staged Proxy nonce.
fn exec_in_box(
    cx: &Cx,
    row: &TaskRow,
    target: &ready::Target,
    exec: &BoxExec,
    spec: &ExecSpec,
) -> Result<ExecOutcome, DriverError> {
    let result = cx.driver().exec(&target.handle, spec, exec.deadline());
    if matches!(result, Err(_) | Ok(ExecOutcome::Interrupted)) {
        exec.cleanup(cx, &site(row, target));
    }
    result
}

/// Where in the Box the Task's commands run.
fn site<'a>(row: &'a TaskRow, target: &'a ready::Target) -> Site<'a> {
    Site {
        handle: &target.handle,
        provider: row.boxed.as_ref().map_or("", |b| b.provider.as_str()),
        workdir: target.workdir.as_ref(),
        worktree: &target.worktree,
        task: Some(row.id),
    }
}

/// The environment every Box exec gets; without the Proxy the command still runs, and warns.
fn box_env(
    cx: &Cx,
    row: &TaskRow,
    target: &ready::Target,
    cfg: &TaskSettings,
) -> Result<BoxExec, CommandError> {
    let proxy = box_exec::proxy(
        cx,
        row.id,
        Some(target.handle.clone()),
        &target.worktree,
        cfg,
    )
    .map_err(|e| {
        cx.warn(Warning {
            code: "proxy_unavailable".to_owned(),
            message: format!("{e}; the command runs without the proxy"),
        });
    })
    .ok();
    box_exec::prepare(cx, &site(row, target), proxy.as_ref(), cfg, Purpose::Run)
        .map_err(|m| CommandError::new(ErrorKind::Failed, m))
}

/// An Idle Task becomes Working for the run; a row that changed since it was read is left as is.
fn mark_working(store: &Store, row: &TaskRow) -> Result<(), CommandError> {
    if row.state != TaskState::Idle {
        return Ok(());
    }
    match store.apply(Decision::new(row.id).task_input(TaskInput::RunStarted)) {
        Ok(_) | Err(StoreError::Illegal(_)) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// The sha the run is stored under and the argv the Box executes.
///
/// A record run is checked first and runs the wrapped script; any other run
/// runs as shell text and takes HEAD when it can be read.
fn plan(
    cx: &Cx,
    req: &RunReq,
    worktree: &Path,
    setup: &[String],
) -> Result<(Option<Sha>, Vec<String>), CommandError> {
    if req.record.is_some() {
        let sha = record::checkout(cx, worktree)?;
        return Ok((Some(sha), record::wrap(setup, &req.argv)));
    }
    let sha = cx
        .git()
        .head(worktree)
        .ok()
        .and_then(|s| Sha::parse(&s).ok());
    let script = match req.argv.as_slice() {
        [text] => text.clone(),
        argv => command_line(argv),
    };
    Ok((sha, vec!["sh".into(), "-c".into(), script]))
}

/// Where the command's output goes, and the log path to report (empty when inherited).
fn output(cx: &Cx, req: &RunReq, id: RunId) -> Result<(Output, String), CommandError> {
    if req.inherit_output {
        return Ok((Output::Inherit, String::new()));
    }
    let dir = cx.state_dir()?.join("runs");
    std::fs::create_dir_all(&dir).map_err(|e| {
        CommandError::new(
            ErrorKind::Failed,
            format!("cannot create {}: {e}", dir.display()),
        )
    })?;
    let path: PathBuf = dir.join(format!("{}.log", id.get()));
    let shown = path.display().to_string();
    Ok((Output::Log(path), shown))
}

/// Ends a run whose command never reported an exit; a store failure leaves it for healing.
fn finish(store: &Store, id: RunId, error: Option<String>) {
    let out = RunOutcome {
        error,
        ..RunOutcome::ended(None, false)
    };
    store.finish_run(id, out).ok();
}

/// The sync mode as the run row stores it.
const fn sync_name(sync: SyncMode) -> &'static str {
    match sync {
        SyncMode::Full => "full",
        SyncMode::None => "none",
    }
}

#[cfg(test)]
mod tests;
