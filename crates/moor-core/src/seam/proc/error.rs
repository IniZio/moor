//! Why a child did not succeed.

use super::Tool;
use std::{fmt, time::Duration};

/// Why a child did not succeed.
#[derive(Debug)]
pub enum ProcError {
    /// The program could not be started.
    Spawn(std::io::Error),
    /// The deadline passed and the child was killed.
    Timeout {
        /// Program that timed out.
        tool: Tool,
        /// Deadline that elapsed.
        after: Duration,
    },
    /// The child died from a signal.
    Signalled(i32),
    /// moor was interrupted and the child was stopped.
    Interrupted,
    /// The child exited non-zero.
    NonZero {
        /// Exit code.
        code: i32,
        /// Captured stdout.
        stdout: String,
        /// Last ~2 KiB of stderr.
        stderr_tail: String,
    },
}

impl fmt::Display for ProcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn(e) => write!(f, "could not start program: {e}"),
            Self::Timeout { tool, after } => {
                write!(f, "{} timed out after {}s", tool.program(), after.as_secs())
            }
            Self::Signalled(s) => write!(f, "program killed by signal {s}"),
            Self::Interrupted => f.write_str("interrupted"),
            Self::NonZero { code, .. } => write!(f, "program exited with code {code}"),
        }
    }
}

impl std::error::Error for ProcError {}
