//! Ordered Box steps run as one batch.

use std::fmt;

use crate::name::{BoxHandle, BoxPath};
use crate::seam::proc::Deadline;

use super::{Driver, DriverError, ExecOutcome, ExecSpec};

/// One step of a Box batch.
#[derive(Clone, PartialEq, Eq)]
pub enum BoxStep {
    /// Reads one file; `StepDone::Peeked(None)` when it is absent.
    Peek(BoxPath),
    /// Runs `sh -c <script>` in the Box with stdin closed; its stdout is returned.
    Run(String),
    /// Writes `bytes` to `path`, creating parent dirs, leaving the file at `mode`.
    /// Content travels on stdin, never argv.
    Put {
        /// Where to write.
        path: BoxPath,
        /// File content.
        bytes: Vec<u8>,
        /// File mode to leave behind.
        mode: u32,
    },
}

impl fmt::Debug for BoxStep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Peek(path) => f.debug_tuple("Peek").field(path).finish(),
            Self::Run(script) => f.debug_tuple("Run").field(script).finish(),
            Self::Put { path, bytes, mode } => f
                .debug_struct("Put")
                .field("path", path)
                .field("mode", mode)
                .field("len", &bytes.len())
                .finish(),
        }
    }
}

/// What one finished [`BoxStep`] produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepDone {
    /// The file content, or `None` when the file is absent.
    Peeked(Option<String>),
    /// A command ran.
    Ran {
        /// How the command ended.
        outcome: ExecOutcome,
        /// What the command wrote to stdout.
        stdout: String,
    },
    /// The file was written.
    Put,
}

/// Runs `steps` one call at a time through the other [`Driver`] methods.
///
/// Stops after a step that errs or a `Run` that does not exit 0, so the result
/// holds one entry per step run, the last being the one that stopped it. The
/// stdout of a `Run` is empty because `Driver::exec` does not return it.
pub fn run_each<D: Driver + ?Sized>(
    driver: &D,
    handle: &BoxHandle,
    steps: &[BoxStep],
    deadline: Deadline,
) -> Vec<Result<StepDone, DriverError>> {
    let mut done = Vec::with_capacity(steps.len());
    for step in steps {
        let result = match step {
            BoxStep::Peek(path) => driver.peek_file(handle, path).map(StepDone::Peeked),
            BoxStep::Run(script) => {
                let spec = ExecSpec {
                    argv: vec!["sh".into(), "-c".into(), script.clone()],
                    env: vec![],
                    cwd: None,
                    output: None,
                    class: None,
                };
                driver
                    .exec(handle, &spec, deadline)
                    .map(|outcome| StepDone::Ran {
                        outcome,
                        stdout: String::new(),
                    })
            }
            BoxStep::Put { path, bytes, mode } => driver
                .put_file(handle, path, bytes, *mode)
                .map(|()| StepDone::Put),
        };
        let stop = match &result {
            Err(_) => true,
            Ok(StepDone::Ran { outcome, .. }) => *outcome != ExecOutcome::Exited { code: 0 },
            Ok(_) => false,
        };
        done.push(result);
        if stop {
            break;
        }
    }
    done
}

#[cfg(test)]
mod tests;
