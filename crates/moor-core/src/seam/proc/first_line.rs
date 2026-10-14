//! Why a long-lived child gave no first stdout line.

use std::fmt;
use std::time::Duration;

use super::Tool;

/// Why a long-lived child gave no first stdout line.
#[derive(Debug)]
pub enum FirstLineError {
    /// No full line arrived before the deadline; the child keeps running.
    Timeout {
        /// Program.
        tool: Tool,
        /// Deadline that elapsed.
        after: Duration,
    },
    /// The child exited before writing a full line.
    Exited {
        /// Exit code, if it exited normally.
        code: Option<i32>,
        /// Signal, if killed by one.
        signal: Option<i32>,
        /// Last ~2 KiB of stderr.
        stderr_tail: String,
    },
    /// Reading the child's stdout failed.
    Io(std::io::Error),
}

impl fmt::Display for FirstLineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout { tool, after } => write!(
                f,
                "{} gave no first line within {}s",
                tool.program(),
                after.as_secs()
            ),
            Self::Exited { code: Some(c), .. } => {
                write!(f, "program exited with code {c} before its first line")
            }
            Self::Exited {
                signal: Some(s), ..
            } => write!(f, "program killed by signal {s} before its first line"),
            Self::Exited { .. } => f.write_str("program exited before its first line"),
            Self::Io(e) => write!(f, "could not read program output: {e}"),
        }
    }
}

impl std::error::Error for FirstLineError {}
