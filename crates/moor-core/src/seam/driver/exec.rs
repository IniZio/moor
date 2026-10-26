//! Commands run inside a Box.

use crate::name::BoxPath;
use crate::seam::proc::Output;

/// A command to run inside a Box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecSpec {
    /// Program and arguments.
    pub argv: Vec<String>,
    /// Extra environment for the command.
    pub env: Vec<(String, String)>,
    /// Directory to run in, made when missing; the Box's login directory when unset.
    pub cwd: Option<BoxPath>,
    /// Where to stream output; captured when unset.
    pub output: Option<Output>,
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
