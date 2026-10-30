//! A user's machine attached to a Box: which machine, and the forwards that let it reach the
//! Box's ports at the same numbers on its own loopback.
//!
//! Each Box port gets a driver local tunnel to a free host loopback port. One held connection to
//! the machine runner then serves every one of them back as the Box port on the machine's
//! loopback.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;

use moor_client::forward::{Forward, Forwarding};
use moor_core::seam::driver::{Driver, LocalTunnel};
use moor_core::seam::proc::Deadline;
use moor_core::BoxHandle;

/// How long the runner may take to answer.
const RUNNER_ANSWER: Deadline = Deadline(std::time::Duration::from_secs(30));

/// Why a machine's forwards are down.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ForwardFailed {
    /// The Box port whose tunnel failed; none when the runner refused.
    pub(crate) port: Option<u16>,
    /// What went wrong.
    pub(crate) error: String,
}

/// The machine runner a machine's agent links in to, and which machine's forwards to ask for.
pub(crate) struct Via<'a> {
    /// The runner's forward socket.
    pub(crate) socket: &'a Path,
    /// The Box whose ports these are.
    pub(crate) box_id: i64,
    /// The machine's enrolled id.
    pub(crate) machine_id: i64,
}

/// The running forwards of one attached machine; dropping it ends every child.
pub(crate) struct MachineLink {
    /// One tunnel per Box port.
    tunnels: Vec<LocalTunnel>,
    /// The connection the runner keeps the agent's listeners up for.
    carrier: UnixStream,
}

impl MachineLink {
    /// Opens a tunnel per port in `ports`, then what serves them on the machine `via`.
    ///
    /// # Errors
    /// The first port whose tunnel did not open, or the runner that did not start the forwards.
    pub(crate) fn open(
        driver: &dyn Driver,
        handle: &BoxHandle,
        via: &Via<'_>,
        ports: &[u16],
    ) -> Result<Self, ForwardFailed> {
        let tunnels = ports
            .iter()
            .map(|&port| {
                driver
                    .local_tunnel(handle, port)
                    .map_err(|e| ForwardFailed {
                        port: Some(port),
                        error: e.to_string(),
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let forwards: Vec<(u16, u16)> = ports
            .iter()
            .zip(&tunnels)
            .map(|(p, t)| (*p, t.host_port))
            .collect();
        let failed = |error: String| ForwardFailed { port: None, error };
        let req = Forward {
            box_id: via.box_id,
            machine_id: via.machine_id,
            ports: forwards,
        };
        let carrier = ask_runner(via.socket, &req).map_err(failed)?;
        Ok(Self { tunnels, carrier })
    }

    /// True while every tunnel and what serves them still run.
    pub(crate) fn alive(&mut self) -> bool {
        let carried = matches!(
            self.carrier.read(&mut [0u8; 1]),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock
        );
        carried && self.tunnels.iter().all(|t| t.tunnel.alive())
    }
}

/// Sends `req` to the runner at `socket`; the connection to hold while the forwards are up.
fn ask_runner(socket: &Path, req: &Forward) -> Result<UnixStream, String> {
    let unreachable = |e: io::Error| {
        format!(
            "cannot reach the machine runner at {}: {e}",
            socket.display()
        )
    };
    let mut conn = UnixStream::connect(socket).map_err(unreachable)?;
    conn.set_read_timeout(Some(RUNNER_ANSWER.0))
        .map_err(unreachable)?;
    let mut line = serde_json::to_vec(req).map_err(|e| e.to_string())?;
    line.push(b'\n');
    conn.write_all(&line).map_err(unreachable)?;
    let mut answer = String::new();
    BufReader::new(&conn)
        .read_line(&mut answer)
        .map_err(unreachable)?;
    match serde_json::from_str(&answer) {
        Ok(Forwarding::Up) => {
            conn.set_nonblocking(true).map_err(unreachable)?;
            Ok(conn)
        }
        Ok(Forwarding::Refused { reason }) => Err(reason),
        Err(_) => Err("the machine runner answered something else".to_owned()),
    }
}

/// One supervisor tick: a link with a dead child is dropped and opened again once.
///
/// # Errors
/// Why the reopened forwards failed; the link is then gone until the next tick.
pub(crate) fn keep(
    link: &mut Option<MachineLink>,
    open: impl FnOnce() -> Result<MachineLink, ForwardFailed>,
) -> Result<(), ForwardFailed> {
    if link.as_mut().is_some_and(MachineLink::alive) {
        return Ok(());
    }
    *link = None;
    *link = Some(open()?);
    Ok(())
}

#[cfg(test)]
mod tests;
