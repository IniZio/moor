//! Reverse-tunnel types shared by the Driver seam.

use std::fmt;
use std::time::Duration;

/// Why a reverse tunnel never became ready.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelFailure {
    /// The tunnel process exited before it reported ready.
    Exited {
        /// Exit code, when the process exited normally.
        code: Option<i32>,
        /// Terminating signal, when a signal ended the process.
        signal: Option<i32>,
    },
    /// No ready report arrived in time; the process was stopped.
    TimedOut(Duration),
}

/// Writes the tunnel failure on one line, with the stderr tail when present.
pub(super) fn fmt_tunnel(
    f: &mut fmt::Formatter<'_>,
    failure: TunnelFailure,
    stderr_tail: &str,
) -> fmt::Result {
    match failure {
        TunnelFailure::Exited { code: Some(c), .. } => {
            write!(f, "reverse tunnel exited before it was ready (exit {c})")?;
        }
        TunnelFailure::Exited {
            signal: Some(s), ..
        } => {
            write!(f, "reverse tunnel exited before it was ready (signal {s})")?;
        }
        TunnelFailure::Exited { .. } => write!(f, "reverse tunnel exited before it was ready")?,
        TunnelFailure::TimedOut(d) => {
            write!(f, "reverse tunnel was not ready after {}s", d.as_secs())?;
        }
    }
    let tail = stderr_tail.trim();
    if tail.is_empty() {
        return Ok(());
    }
    write!(f, ": {}", tail.replace('\n', " "))
}

/// A running Box-to-host reverse tunnel. Dropping it stops the tunnel.
pub trait TunnelHandle: Send {
    /// Host pid of the tunnel process.
    fn pid(&self) -> u32;
    /// Whether the tunnel process is still running.
    fn alive(&self) -> bool;
}
