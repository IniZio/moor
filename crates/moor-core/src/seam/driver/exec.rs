//! Commands run inside a Box.

use crate::name::BoxPath;
use crate::seam::driver::Place;
use crate::seam::proc::Output;

/// A command to run inside a Box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecSpec {
    /// Which place in the Box runs the command.
    pub place: Place,
    /// Program and arguments.
    pub argv: Vec<String>,
    /// Extra environment for the command.
    pub env: Vec<(String, String)>,
    /// Directory to run in, made when missing; the Box's login directory when unset.
    pub cwd: Option<BoxPath>,
    /// Where to stream output; captured when unset.
    pub output: Option<Output>,
    /// Whether the command is safe to run twice, so a driver may run it again when a lost link
    /// leaves unknown whether it ran. Off unless moor marks it.
    pub repeatable: bool,
}

impl ExecSpec {
    /// A command for `place` with no extra environment, the Box's login directory, and its
    /// output captured.
    #[must_use]
    pub const fn new(place: Place, argv: Vec<String>) -> Self {
        Self {
            place,
            argv,
            env: Vec::new(),
            cwd: None,
            output: None,
            repeatable: false,
        }
    }

    /// Marks the command safe to run twice.
    #[must_use]
    pub fn repeatable(self) -> Self {
        Self {
            repeatable: true,
            ..self
        }
    }

    /// Sets the extra environment; `tools/exec-env.sh` limits what may be passed.
    #[must_use]
    pub fn env(self, env: Vec<(String, String)>) -> Self {
        Self { env, ..self }
    }

    /// Sets the directory to run in.
    #[must_use]
    pub fn cwd(self, cwd: Option<BoxPath>) -> Self {
        Self { cwd, ..self }
    }

    /// Streams output to `output` instead of capturing it.
    #[must_use]
    pub fn output(self, output: Option<Output>) -> Self {
        Self { output, ..self }
    }
}

/// How a command inside a Box ended; a non-zero code is not an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecOutcome {
    /// The command finished with this exit code.
    Exited {
        /// Exit code of the command.
        code: i32,
    },
    /// The command was stopped by an interrupt or signal before it finished.
    Interrupted,
}
