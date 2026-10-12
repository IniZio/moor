//! Runs one command in a Box through `crabbox run --no-sync`.

use moor_core::name::BoxHandle;
use moor_core::seam::driver::{DriverError, ExecOutcome, ExecSpec};
use moor_core::seam::proc::Deadline;

use super::errors::{map_error, remote_exit, Op};
use super::{split_handle, static_env, target_args, Crabbox};

/// Shell wrapper that enters the directory in `$1`, then runs the rest.
const CD_THEN_EXEC: &str = r#"cd "$1" && shift && exec "$@""#;

/// Runs `spec` in the Box behind `handle`.
///
/// Env travels in the child's environment and is named by `--allow-env`, so
/// values stay out of argv. A `cwd` is entered by a `sh -c` wrapper because
/// `crabbox run` has no remote directory flag; without one it runs in the
/// Box workdir.
pub(super) fn run(
    cb: &Crabbox,
    handle: &BoxHandle,
    spec: &ExecSpec,
    deadline: Deadline,
) -> Result<ExecOutcome, DriverError> {
    let mut proc = Crabbox::spec(deadline.0)
        .arg("run")
        .args(target_args(handle))
        .arg("--no-sync");
    for (k, v) in &spec.env {
        proc = proc.args(["--allow-env", k]).env(k, v);
    }
    proc = static_env(proc, handle).arg("--");
    if let Some(cwd) = &spec.cwd {
        proc = proc.args(["sh", "-c", CD_THEN_EXEC, "sh", cwd.as_str()]);
    }
    proc = proc.args(&spec.argv);
    match cb.proc.run(&proc) {
        Ok(_) => Ok(ExecOutcome { code: 0 }),
        Err(e) => remote_exit(&e).map_or_else(
            || Err(map_error(Op::Exec, split_handle(handle).0, &e)),
            |code| Ok(ExecOutcome { code }),
        ),
    }
}
