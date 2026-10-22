//! `run`: execute a command on the Box's machine, keep a run record, report how it ended.
//!
//! The Box lock is held from the run row's creation to its completion. So is the run's follow
//! claim, which is what marks the row in flight to every other reader.

use std::path::PathBuf;

use moor_core::boxscript::record::HEAD_MOVED;
use moor_core::boxscript::{command_line, shell_argv};
use moor_core::config::{BoxSettings, Flags};
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::boxes::BoxState;
use moor_core::domain::decision::Decision;
use moor_core::domain::session_state::SessionInput;
use moor_core::exec_env::Purpose;
use moor_core::seam::clock::Ts;
use moor_core::seam::driver::{ExecSpec, Launched};
use moor_core::seam::proc::Deadline;
use moor_core::seam::proc::Output;
use moor_core::view::run::RunValue;
use moor_core::{BoxPath, ErrorKind, RunId, RunReq, Sha};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::box_exec::{self, BoxExec, Site};
use crate::ops::common::boxlock::{self, BoxLock};
use crate::ops::common::facts::settings_of;
use crate::ops::common::provisioning::dead_provisioning;
use crate::ops::common::wake::wake_and_revive;
use crate::store::{BoxRow, NewRun, RunOutcome, Store, StoreError};

mod detached;
mod follow;
mod ready;
mod record;

pub(super) use follow::Sink;

/// Run `run`.
///
/// # Errors
/// [`CommandError`] when the command could not start: unknown Box, no usable machine, a busy machine,
/// a `.moor.yaml` that differs from the base commit without trust, or a failure of the driver or the store before the command ran.
pub(crate) fn run(cx: &Cx, req: &RunReq) -> Result<RunValue, CommandError> {
    streamed(cx, req, None)
}

/// [`run`], passing the command's output to `sink` as it reaches the run log.
///
/// # Errors
/// As [`run`].
pub(crate) fn streamed(
    cx: &Cx,
    req: &RunReq,
    sink: Option<Sink<'_>>,
) -> Result<RunValue, CommandError> {
    if req.argv.is_empty() {
        return Err(CommandError::new(ErrorKind::Usage, "no command to run"));
    }
    let store = cx.store()?;
    let name = req.box_name.as_str();
    let found = store
        .latest_box_by_name(&req.box_name)?
        .ok_or_else(|| CommandError::new(ErrorKind::Failed, format!("no Box {name}")))?;
    dead_provisioning(cx, &found)?;
    let _held = BoxLock::acquire(cx.clock(), cx.state_dir()?, found.id, "run", boxlock::WAIT)?;
    let row = store
        .box_row(found.id)?
        .ok_or_else(|| CommandError::new(ErrorKind::Failed, format!("no Box {name}")))?;
    locked(cx, store, req, &row, sink)
}

/// The part of `run` that needs the Box lock, on the row read after taking it.
fn locked(
    cx: &Cx,
    store: &Store,
    req: &RunReq,
    row: &BoxRow,
    sink: Option<Sink<'_>>,
) -> Result<RunValue, CommandError> {
    dead_provisioning(cx, row)?;
    let gone = row
        .boxed
        .as_ref()
        .is_some_and(|b| matches!(b.state, BoxState::Gone | BoxState::Deleted));
    let woken;
    let row = if !gone && wake_and_revive(cx, store, row)? {
        woken = store.box_row(row.id)?.ok_or_else(|| {
            CommandError::new(ErrorKind::Failed, format!("no Box {}", row.name.as_str()))
        })?;
        &woken
    } else {
        row
    };
    let target = ready::target(store, row)?;
    launch(cx, store, req, row, &target, sink)
}

/// Records the run, executes the command in the Box and records how it ended.
fn launch(
    cx: &Cx,
    store: &Store,
    req: &RunReq,
    row: &BoxRow,
    target: &ready::Target,
    sink: Option<Sink<'_>>,
) -> Result<RunValue, CommandError> {
    let cfg = settings_of(cx, row, &Flags::default())?;
    let (sha, argv) = plan(cx, req, target, &cfg.setup)?;
    let (id, _claim) = store.begin_run(NewRun {
        box_id: row.id,
        argv: req.argv.clone(),
        record: req.record.clone(),
        sha: sha.clone(),
        pid: std::process::id(),
    })?;
    mark_working(store, row).inspect_err(|_| finish(store, id, None))?;
    let (output, log) = output(cx, req, id).inspect_err(|_| finish(store, id, None))?;
    let exec = box_env(cx, row, target, &cfg)
        .inspect_err(|e| finish(store, id, Some(e.error.to_string())))?;
    let spec = ExecSpec {
        argv: exec.argv(argv),
        env: exec.vars(),
        cwd: exec.cwd(),
        output: None,
    };
    let (workdir, until) = start(
        cx,
        store,
        target,
        (id, sha.is_some()),
        &spec,
        exec.deadline(),
    )
    .inspect_err(|e| finish(store, id, Some(e.error.to_string())))?;
    let followed = follow::during(Some(&output), sink, || {
        follow_into(cx, target, workdir.as_ref(), id, until, &output)
    })
    .inspect_err(|e| finish(store, id, Some(e.error.to_string())))?;
    let (exit, interrupted) = followed.exit();
    if let Some(error) = followed.error() {
        let out = RunOutcome {
            error: Some(error.clone()),
            ..RunOutcome::ended(exit, interrupted)
        };
        store.finish_run(id, out)?;
        return Err(CommandError::new(ErrorKind::Failed, error));
    }
    let moved = sha.is_some() && exit.is_some_and(|c| c != 0) && followed.moved();
    store.finish_run(id, ended(exit, interrupted, moved))?;
    Ok(RunValue {
        ran: true,
        run: Some(id),
        exit,
        interrupted,
        sha: sha.map(|s| s.as_str().to_owned()),
        record: req.record.clone(),
        log,
    })
}

/// Starts run `id` detached in the Box and records its Box pid. Returns the Box workdir its end
/// looks for the moved marker in, which a record run must know, and when its deadline passes.
fn start(
    cx: &Cx,
    store: &Store,
    target: &ready::Target,
    (id, record): (RunId, bool),
    spec: &ExecSpec,
    deadline: Deadline,
) -> Result<(Option<BoxPath>, Ts), CommandError> {
    let failed = |m: String| CommandError::new(ErrorKind::Failed, m);
    let workdir = if record {
        Some(record::workdir(cx, target).map_err(failed)?)
    } else {
        target.workdir.clone()
    };
    let started = store
        .run(id)?
        .ok_or_else(|| failed(format!("run {} has no row", id.get())))?
        .started_at;
    match cx.driver().launch_run(&target.handle, id.get(), spec) {
        Ok(Launched::Started { pid }) => store.set_run_pid(id, pid)?,
        Ok(Launched::AlreadyLaunched) => {
            return Err(failed(format!(
                "the Box already holds a run {}; use a fresh state dir or a fresh Box",
                id.get()
            )))
        }
        Err(e) => return Err(failed(e.to_string())),
    }
    Ok((workdir, started.saturating_add(deadline.0)))
}

/// Follows run `id` into the host side of `output` until it ends; `workdir` is where its end
/// looks for the moved marker.
fn follow_into(
    cx: &Cx,
    target: &ready::Target,
    workdir: Option<&BoxPath>,
    id: RunId,
    until: Ts,
    output: &Output,
) -> Result<detached::Ended, CommandError> {
    let Output::Log(path) = output else {
        let mut out = std::io::stdout();
        return Ok(detached::follow(
            cx,
            target,
            workdir,
            id,
            until,
            (0, &mut out),
        ));
    };
    let opened = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|f| Ok((f.metadata()?.len(), f)));
    let (from, mut file) = opened.map_err(|e| {
        CommandError::new(
            ErrorKind::Failed,
            format!("cannot open {}: {e}", path.display()),
        )
    })?;
    Ok(detached::follow(
        cx,
        target,
        workdir,
        id,
        until,
        (from, &mut file),
    ))
}

/// Where in the machine the Box's commands run.
fn site<'a>(row: &'a BoxRow, target: &'a ready::Target) -> Site<'a> {
    Site {
        handle: &target.handle,
        provider: row.boxed.as_ref().map_or("", |b| b.provider.as_str()),
        workdir: target.workdir.as_ref(),
    }
}

/// The environment every Box exec gets; a Proxy that will not start fails the run.
fn box_env(
    cx: &Cx,
    row: &BoxRow,
    target: &ready::Target,
    cfg: &BoxSettings,
) -> Result<BoxExec, CommandError> {
    let proxy = box_exec::proxy(
        cx,
        row.id,
        Some(target.handle.clone()),
        row.repo.as_ref().map(|r| &r.repo),
        cfg,
    )
    .map_err(|e| {
        CommandError::new(
            ErrorKind::Failed,
            format!("the Proxy could not start: {e}; retry `moor run` once it starts"),
        )
    })?;
    box_exec::prepare(cx, &site(row, target), Some(&proxy), cfg, Purpose::Run)
        .map_err(|m| CommandError::new(ErrorKind::Failed, m))
}

/// An Idle Box becomes Working for the run; a row that changed since it was read is left as is.
fn mark_working(store: &Store, row: &BoxRow) -> Result<(), CommandError> {
    if !(row.lifecycle == BoxLifecycle::Ready && row.session.is_quiet()) {
        return Ok(());
    }
    match store.apply(Decision::new(row.id).session(SessionInput::RunStarted)) {
        Ok(_) | Err(StoreError::Illegal(_)) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// The sha the run is stored under and the argv the Box executes.
///
/// A record run reads the Box HEAD first and runs the wrapped script; any other run runs as
/// shell text and is stored under no sha.
fn plan(
    cx: &Cx,
    req: &RunReq,
    target: &ready::Target,
    setup: &[String],
) -> Result<(Option<Sha>, Vec<String>), CommandError> {
    if req.record.is_some() {
        let sha = record::checkout(cx, target)?;
        let argv = record::wrap(setup, &req.argv, &sha);
        return Ok((Some(sha), argv));
    }
    let script = match req.argv.as_slice() {
        [text] => text.clone(),
        argv => command_line(argv),
    };
    Ok((None, shell_argv(&script)))
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

/// How a run whose command reported back ended; a record run whose checkout moved carries why.
fn ended(exit: Option<i32>, interrupted: bool, moved: bool) -> RunOutcome {
    RunOutcome {
        error: moved.then(|| HEAD_MOVED.to_owned()),
        ..RunOutcome::ended(exit, interrupted)
    }
}

/// Ends a run whose command never reported an exit; a store failure leaves it for healing.
fn finish(store: &Store, id: RunId, error: Option<String>) {
    let out = RunOutcome {
        error,
        ..RunOutcome::ended(None, false)
    };
    store.finish_run(id, out).ok();
}

#[cfg(test)]
pub(crate) mod tests;
