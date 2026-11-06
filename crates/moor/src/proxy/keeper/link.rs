//! A user's machine attached to a Box: which machine, and the forwards that let it reach the
//! Box's ports at the same numbers on its own loopback.
//!
//! Each Box port gets a driver local tunnel to a free host loopback port. One held connection to
//! the machine runner then serves every one of them back as the Box port on the machine's
//! loopback.

use std::io::{self, BufRead, BufReader, Read};

use moor_client::forward::{Forward, Forwarding};
use moor_client::link::reach::{Runner, Service, StdConn};
use moor_core::seam::driver::{Driver, LocalTunnel};
use moor_core::seam::proc::Deadline;
use moor_core::BoxHandle;

/// How long the runner may take to answer.
const RUNNER_ANSWER: Deadline = Deadline(std::time::Duration::from_secs(30));
/// How long a connect to the runner may take; a tick waits on it under the keeper's busy lock.
const RUNNER_CONNECT: std::time::Duration = std::time::Duration::from_secs(2);

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
    /// The runner that holds the machine's link, its name already looked up.
    pub(crate) runner: &'a Runner,
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
    carrier: StdConn,
    /// The runner it is open on.
    runner: Runner,
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
        let carrier = ask_runner(via.runner, &req).map_err(failed)?;
        Ok(Self {
            tunnels,
            carrier,
            runner: via.runner.clone(),
        })
    }

    /// True while every tunnel and what serves them still run.
    pub(crate) fn alive(&mut self) -> bool {
        let carried = matches!(
            self.carrier.read(&mut [0u8; 1]),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock
        );
        carried && self.tunnels.iter().all(|t| t.tunnel.alive())
    }

    /// True while it is alive and open on `now`, the runner the link row names; an unknown
    /// runner keeps it, so a store hiccup cuts no forwards.
    pub(crate) fn holds(&mut self, now: Option<&Runner>) -> bool {
        self.alive() && now.is_none_or(|now| *now == self.runner)
    }
}

/// Sends `req` to `runner`; the connection to hold while the forwards are up.
fn ask_runner(runner: &Runner, req: &Forward) -> Result<StdConn, String> {
    let unreachable = |e: io::Error| format!("the machine runner went away: {e}");
    let mut conn = crate::proxy::links::connect(runner, Service::Forwards, RUNNER_CONNECT)?;
    conn.set_read_timeout(Some(RUNNER_ANSWER.0))
        .map_err(unreachable)?;
    let mut line = serde_json::to_vec(req).map_err(|e| e.to_string())?;
    line.push(b'\n');
    io::Write::write_all(&mut conn, &line).map_err(unreachable)?;
    let mut answer = String::new();
    BufReader::new(&mut conn)
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

/// One keeper tick: a link with a dead child, or open on another runner than `now`, is dropped
/// and opened again once.
///
/// # Errors
/// Why the reopened forwards failed; the link is then gone until the next tick.
pub(crate) fn keep(
    link: &mut Option<MachineLink>,
    now: Option<&Runner>,
    open: impl FnOnce() -> Result<MachineLink, ForwardFailed>,
) -> Result<(), ForwardFailed> {
    if link.as_mut().is_some_and(|l| l.holds(now)) {
        return Ok(());
    }
    *link = None;
    *link = Some(open()?);
    Ok(())
}

#[cfg(test)]
mod tests;
