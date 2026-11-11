//! The fake's port forwards: a Box is on no node until a test puts it on one, and a node's Box
//! echoes what a forward sends it.

use std::io::Read;
use std::sync::mpsc;
use std::time::Duration;

use moor_core::seam::driver::{DriverError, Forwards, NO_PORTS};
use moor_core::seam::proc::Pipes;
use moor_core::BoxHandle;

use super::FakeDriver;

/// What the fake's Boxes do with ports.
#[derive(Default)]
pub(super) struct PortState {
    /// Whether the Boxes are on a node that forwards ports.
    on_a_node: bool,
    /// The ports something listens on.
    listening: Vec<u16>,
    /// Fails every port call.
    fail: Option<DriverError>,
    /// The bytes the Box's ports received.
    received: Vec<u8>,
}

impl Forwards for FakeDriver {
    fn port_listens(&self, handle: &BoxHandle, port: u16) -> Result<bool, DriverError> {
        self.port_listening(handle, port)
    }

    fn forward_piped(
        &self,
        handle: &BoxHandle,
        port: u16,
        pipes: Pipes<'_>,
    ) -> Result<(), DriverError> {
        self.port_forwarded(handle, port, pipes)
    }
}

impl FakeDriver {
    /// This driver's Boxes sit on a node that forwards ports, with something listening on
    /// each of `listening`.
    pub(crate) fn on_a_node(self, listening: &[u16]) -> Self {
        {
            let mut s = self.st();
            s.ports.on_a_node = true;
            s.ports.listening = listening.to_vec();
        }
        self
    }

    /// Every port call fails with `e`.
    pub(crate) fn failing_ports(self, e: DriverError) -> Self {
        self.st().ports.fail = Some(e);
        self
    }

    /// The bytes forwards have sent into the Box's ports.
    pub(crate) fn port_bytes(&self) -> Vec<u8> {
        self.st().ports.received.clone()
    }

    /// Whether something listens on `port`.
    pub(super) fn port_listening(
        &self,
        handle: &BoxHandle,
        port: u16,
    ) -> Result<bool, DriverError> {
        let mut s = self.st();
        s.note(format!("port_listens {} {port}", handle.as_str()));
        if let Some(e) = s.ports.fail.clone() {
            return Err(e);
        }
        if !s.ports.on_a_node {
            return Err(DriverError::Unsupported(NO_PORTS));
        }
        Ok(s.ports.listening.contains(&port))
    }

    /// Echoes the connection's bytes back until its input ends or `pipes.stop` says to cut it.
    pub(super) fn port_forwarded(
        &self,
        handle: &BoxHandle,
        port: u16,
        pipes: Pipes<'_>,
    ) -> Result<(), DriverError> {
        self.port_listening(handle, port)
            .map(drop)
            .and_then(|()| self.note_forward(handle, port))?;
        let Pipes {
            mut stdin,
            stdout,
            stop,
            ..
        } = pipes;
        let (chunks, rx) = mpsc::channel::<Vec<u8>>();
        // Detached: a read of the connection's input may block after the forward was cut.
        std::thread::spawn(move || {
            let mut buf = [0u8; 1024];
            while let Ok(n @ 1..) = stdin.read(&mut buf) {
                if chunks
                    .send(buf.get(..n).unwrap_or_default().to_vec())
                    .is_err()
                {
                    return;
                }
            }
        });
        loop {
            if stop() {
                return Ok(());
            }
            match rx.recv_timeout(Duration::from_millis(10)) {
                Ok(chunk) => {
                    self.st().ports.received.extend_from_slice(&chunk);
                    if stdout
                        .write_all(&chunk)
                        .and_then(|()| stdout.flush())
                        .is_err()
                    {
                        return Ok(());
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
            }
        }
    }

    /// Notes a forward to `port`, which needs something listening there.
    fn note_forward(&self, handle: &BoxHandle, port: u16) -> Result<(), DriverError> {
        let mut s = self.st();
        s.note(format!("forward {} {port}", handle.as_str()));
        if s.ports.listening.contains(&port) {
            Ok(())
        } else {
            Err(DriverError::Failed {
                summary: format!("nothing listens on port {port}"),
            })
        }
    }
}
