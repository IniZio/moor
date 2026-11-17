//! What a driver does for a port forward.

use super::DriverError;
use crate::name::BoxHandle;
use crate::seam::proc::Pipes;

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
