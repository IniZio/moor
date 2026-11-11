//! Reverse-tunnel types shared by the Driver seam.

use std::fmt;
use std::time::Duration;

use super::DriverError;
use crate::name::BoxHandle;
use crate::seam::proc::Pipes;

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

/// A host loopback port forwarded to a port inside a Box; the forward ends when `tunnel` drops.
pub struct LocalTunnel {
    /// The host loopback port that reaches the Box port.
    pub host_port: u16,
    /// The running forward.
    pub tunnel: Box<dyn TunnelHandle>,
}

impl std::fmt::Debug for LocalTunnel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalTunnel")
            .field("host_port", &self.host_port)
            .field("pid", &self.tunnel.pid())
            .finish()
    }
}

/// What [`DriverError::Unsupported`] names for a Box that is not on a node that forwards ports.
pub const NO_PORTS: &str = "forwarding a port";

/// What [`DriverError::Unsupported`] names for a node whose agent predates port forwards.
pub const OLD_AGENT_PORTS: &str = "forwarding a port through an agent that predates it";

/// What a driver does for a port forward; a driver that does none keeps these answers.
pub trait Forwards {
    /// Whether anything listens on `port` of the Box's loopback.
    ///
    /// # Errors
    /// [`DriverError::Unsupported`] when the Box is not on a node that forwards ports; another
    /// [`DriverError`] when the node cannot say.
    fn port_listens(&self, handle: &BoxHandle, port: u16) -> Result<bool, DriverError> {
        let _ = (handle, port);
        Err(DriverError::Unsupported(NO_PORTS))
    }

    /// Carries one TCP connection to `port` of the Box's loopback through `pipes`, byte for
    /// byte: `pipes.stdin` goes in, what the port sends comes out on `pipes.stdout`, and
    /// `pipes.stderr` is unused. It returns once either side ends the connection or `pipes.stop`
    /// says to cut it.
    ///
    /// # Errors
    /// [`DriverError::Unsupported`] when the Box is not on a node that forwards ports; another
    /// [`DriverError`] when the connection cannot be made.
    fn forward_piped(
        &self,
        handle: &BoxHandle,
        port: u16,
        pipes: Pipes<'_>,
    ) -> Result<(), DriverError> {
        let _ = (handle, port, pipes);
        Err(DriverError::Unsupported(NO_PORTS))
    }
}
