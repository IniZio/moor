//! Opt-in timing of crabbox invocations, one stderr line per call.
//!
//! Only the subcommand is printed: the remaining arguments may carry tokens.

use std::ffi::OsString;
use std::io::{self, Write};
use std::time::{Duration, Instant};

use moor_core::seam::proc::{ProcError, ProcSpec, Tool};

use super::plain::last_sentence;
use crate::env::Env;

/// Variable that turns tracing on when set to `1`.
pub(super) const TRACE_VAR: &str = "MOOR_TRACE_CRABBOX";

/// Whether crabbox calls are traced.
#[derive(Debug, Clone, Copy)]
pub(super) struct Trace {
    /// [`TRACE_VAR`] was `1` in moor's environment.
    enabled: bool,
}

impl Trace {
    /// Reads [`TRACE_VAR`] from `env`.
    pub(super) fn new(env: &Env) -> Self {
        Self {
            enabled: env.get(TRACE_VAR) == Some("1"),
        }
    }

    /// Whether a call to `spec` is traced.
    fn active(self, spec: &ProcSpec) -> bool {
        self.enabled && spec.tool == Tool::Crabbox
    }

    /// Runs `call`, then reports its duration and exit when `spec` is traced.
    #[expect(
        clippy::disallowed_methods,
        reason = "diagnostic timing of a child, not domain time"
    )]
    pub(super) fn timed<T>(
        self,
        spec: &ProcSpec,
        call: impl FnOnce() -> Result<T, ProcError>,
    ) -> Result<T, ProcError> {
        if !self.active(spec) {
            return call();
        }
        let begun = Instant::now();
        let result = call();
        let exit = exit_label(result.as_ref().err());
        let line = with_said(
            done_line(&spec.args, begun.elapsed(), &exit),
            result.as_ref().err(),
        );
        emit(&mut io::stderr(), &line);
        result
    }

    /// Reports that a long-lived `spec` child started.
    pub(super) fn started(self, spec: &ProcSpec) {
        if self.active(spec) {
            emit(&mut io::stderr(), &started_line(&spec.args));
        }
    }
}

/// First argument that is not a flag, or `--version` when that is the call.
fn subcommand(args: &[OsString]) -> String {
    if let Some(a) = args.iter().find(|a| !a.to_string_lossy().starts_with('-')) {
        return a.to_string_lossy().into_owned();
    }
    match args.first() {
        Some(a) if a == "--version" => "--version".to_owned(),
        _ => "-".to_owned(),
    }
}

/// `0`, the exit code, or a word for the other ways a call ends.
fn exit_label(err: Option<&ProcError>) -> String {
    match err {
        None => "0".to_owned(),
        Some(ProcError::NonZero { code, .. }) => code.to_string(),
        Some(ProcError::Signalled(_)) => "signal".to_owned(),
        Some(ProcError::Spawn(_)) => "spawn-error".to_owned(),
        Some(ProcError::Timeout { .. }) => "timeout".to_owned(),
        Some(ProcError::Interrupted) => "interrupted".to_owned(),
    }
}

/// Line for a finished call.
fn done_line(args: &[OsString], took: Duration, exit: &str) -> String {
    format!(
        "moor-trace: crabbox {} took={:.3}s exit={exit}",
        subcommand(args),
        took.as_secs_f64()
    )
}

/// `line`, plus what a non-zero exit last printed, in plain words.
fn with_said(line: String, err: Option<&ProcError>) -> String {
    let said = match err {
        Some(ProcError::NonZero { stderr_tail, .. }) => last_sentence(stderr_tail),
        _ => None,
    };
    match said {
        Some(said) => format!("{line} said={said:?}"),
        None => line,
    }
}

/// Line for a long-lived child.
fn started_line(args: &[OsString]) -> String {
    format!("moor-trace: crabbox {} started", subcommand(args))
}

/// Writes `line`; a failed write never fails the call.
fn emit(w: &mut dyn Write, line: &str) {
    drop(writeln!(w, "{line}"));
}

#[cfg(test)]
mod tests;
