//! Runs one command in a Box through `crabbox run`.

use moor_core::name::BoxHandle;
use moor_core::seam::driver::{DriverError, ExecOutcome, ExecSpec};
use moor_core::seam::proc::{Deadline, ProcError, StderrGate};

use super::errors::{map_error, remote_exit, Op};
use super::{size_of, split_handle, static_env, target_args, Crabbox};

/// Shell wrapper that enters the directory in `$1`, then runs the rest.
const CD_THEN_EXEC: &str = r#"cd "$1" && shift && exec "$@""#;

/// Shell wrapper that brackets the command's stderr in two marker lines.
///
/// `$1` and `$2` are the begin and end markers; the rest is the command. It
/// is not `exec`ed so the end marker prints after, and the command's exit
/// status is kept. Crabbox's own stderr lies outside the markers.
pub(super) const STDERR_GATE: &str =
    r#"b=$1 e=$2; shift 2; printf '%s\n' "$b" >&2; "$@"; c=$?; printf '%s\n' "$e" >&2; exit $c"#;

/// Runs `spec` in the Box behind `handle`.
///
/// crabbox never syncs the host checkout into the Box and its own hydrate
/// step never runs; the Box clones its repo itself. With an `output` the
/// child streams there instead of being captured.
///
/// Env travels in the child's environment and is named by `--allow-env`, so
/// values stay out of argv. The command always runs under a `sh -c` wrapper;
/// a `cwd` is entered there because `crabbox run` has no remote directory
/// flag, and without one it runs in the Box workdir. A streamed run also
/// wraps the command in `STDERR_GATE` with random markers, so the runner can
/// drop crabbox's own stderr around it.
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
        .arg("--no-sync");
    if let Some(class) = spec.class.as_deref() {
        proc = proc.args(["--class", size_of(Some(class))?]);
    }
    if let Some(dir) = &spec.host_cwd {
        proc = proc.cwd(dir);
    }
    for (k, v) in &spec.env {
        proc = proc.args(["--allow-env", k]).env(k, v);
    }
    proc = static_env(proc, handle).arg("--");
    if spec.output.is_some() {
        let token = format!("gate-{:08x}{:08x}", cb.rng.u32(), cb.rng.u32());
        let (begin, end) = (format!("{token}-begin"), format!("{token}-end"));
        proc = proc.args(["sh", "-c", STDERR_GATE, "sh", &begin, &end]);
        proc = proc.stderr_gate(StderrGate { begin, end });
    }
    if let Some(cwd) = &spec.cwd {
        proc = proc.args(["sh", "-c", CD_THEN_EXEC, "sh", cwd.as_str()]);
    }
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
