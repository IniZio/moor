//! Runs one command in a Box through `crabbox run`.

use moor_core::domain::run::SyncMode;
use moor_core::name::BoxHandle;
use moor_core::seam::driver::{DriverError, ExecOutcome, ExecSpec};
use moor_core::seam::proc::{Deadline, ProcError};

use super::errors::{map_error, remote_exit, Op};
use super::{size_of, split_handle, static_env, target_args, Crabbox};

/// Unsets the shell variables crabbox's workspace wrapper exports before it
/// execs, so they stay out of the command's environment.
macro_rules! unset_workspace_vars {
    () => {
        "unset -f owner_gate_function 2>/dev/null\n\
unset -v state child gate token command_umask run_dir owner_gate_function child_pid child_identity 2>/dev/null\n"
    };
}

/// Shell wrapper that enters the directory in `$1`, then runs the rest.
const CD_THEN_EXEC: &str = concat!(unset_workspace_vars!(), r#"cd "$1" && shift && exec "$@""#);

/// Shell wrapper that runs its arguments with the workspace variables unset.
pub(super) const UNSET_THEN_EXEC: &str = concat!(unset_workspace_vars!(), r#"exec "$@""#);

/// Runs `spec` in the Box behind `handle`.
///
/// The worktree is synced only for `SyncMode::Full`, seeded from the local
/// git state; crabbox's own hydrate step never runs. With an `output` the
/// child streams there instead of being captured.
///
/// Env travels in the child's environment and is named by `--allow-env`, so
/// values stay out of argv. The command always runs under a `sh -c` wrapper;
/// a `cwd` is entered there because `crabbox run` has no remote directory
/// flag, and without one it runs in the Box workdir.
pub(super) fn run(
    cb: &Crabbox,
    handle: &BoxHandle,
    spec: &ExecSpec,
    deadline: Deadline,
) -> Result<ExecOutcome, DriverError> {
    let mut proc = Crabbox::spec(deadline.0)
        .arg("run")
        .args(target_args(handle))
        .arg("--no-hydrate")
        .env("CRABBOX_SYNC_GIT_SEED_SOURCE", "local");
    if spec.sync == SyncMode::None {
        proc = proc.arg("--no-sync");
    }
    if let Some(class) = spec.class.as_deref() {
        proc = proc.args(["--class", size_of(Some(class))?.0]);
    }
    if let Some(dir) = &spec.host_cwd {
        proc = proc.cwd(dir);
    }
    for (k, v) in &spec.env {
        proc = proc.args(["--allow-env", k]).env(k, v);
    }
    proc = static_env(proc, handle).arg("--");
    proc = match &spec.cwd {
        Some(cwd) => proc.args(["sh", "-c", CD_THEN_EXEC, "sh", cwd.as_str()]),
        None => proc.args(["sh", "-c", UNSET_THEN_EXEC, "sh"]),
    };
    proc = proc.args(&spec.argv);
    let result = cb.retry_claim_race(|| {
        spec.output.as_ref().map_or_else(
            || cb.proc.run(&proc).map(|_| 0),
            |out| cb.proc.run_streamed(&proc, out).map(|()| 0),
        )
    });
    match result {
        Ok(code) => Ok(ExecOutcome::Exited { code }),
        Err(ProcError::Interrupted | ProcError::Signalled(_)) => Ok(ExecOutcome::Interrupted),
        Err(e) => remote_exit(&e).map_or_else(
            || Err(map_error(Op::Exec, split_handle(handle).0, &e)),
            |code| Ok(ExecOutcome::Exited { code }),
        ),
    }
}

#[cfg(test)]
mod tests;
