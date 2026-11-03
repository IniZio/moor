//! The fake's streaming exec: runs the command on the host, without the Box env's prelude,
//! which writes the Proxy CA and must never touch the host.

use std::process::{Command, Stdio};

use moor_core::seam::driver::{DriverError, ExecOutcome, ExecSpec};
use moor_core::seam::proc::{pump, Deadline, Pipes};
use moor_core::BoxHandle;

use super::FakeDriver;

/// How a streaming exec ended.
pub(super) type Ended = Result<ExecOutcome, DriverError>;

impl FakeDriver {
    /// Records `spec` as an exec, then runs the command it wraps with its stdio on `pipes`.
    pub(super) fn piped(&self, handle: &BoxHandle, spec: &ExecSpec, pipes: Pipes<'_>) -> Ended {
        {
            let mut s = self.st();
            s.note(format!("exec_piped {}", handle.as_str()));
            s.execs.push((
                handle.clone(),
                spec.clone(),
                Deadline(std::time::Duration::ZERO),
            ));
            if !s.live.iter().any(|h| h == handle.as_str()) {
                return Err(DriverError::NotFound);
            }
        }
        let argv = match spec.argv.as_slice() {
            [sh, c, _prelude, name, rest @ ..] if sh == "sh" && c == "-c" && name == "sh" => rest,
            all => all,
        };
        let [program, words @ ..] = argv else {
            return Err(DriverError::Failed {
                summary: "the box command is empty".to_owned(),
            });
        };
        let child = Command::new(program)
            .args(words)
            .envs(spec.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| DriverError::Unavailable(e.to_string()))?;
        let ended = pump(child, Vec::new(), pipes).map_err(|e| DriverError::Failed {
            summary: e.to_string(),
        })?;
        Ok(match (ended.code, ended.stopped) {
            (Some(code), false) => ExecOutcome::Exited { code },
            _ => ExecOutcome::Interrupted,
        })
    }
}
