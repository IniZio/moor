//! A user's machine attached to a Box: which machine, and the forwards that let it reach the
//! Box's ports at the same numbers on its own loopback.
//!
//! Each Box port gets a driver local tunnel to a free host loopback port. One held connection to
//! the machine runner, the carrier, has the machine listen on every Box port and names each
//! connection the machine accepts; the keeper dials the runner back for it and pipes it to the
//! port's tunnel.

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader};
use std::net::{Ipv4Addr, Shutdown, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use moor_client::forward::{Carried, Forward, Forwarding};
use moor_client::link::reach::{Runner, Service, StdConn};
use moor_core::seam::driver::{Driver, LocalTunnel};
use moor_core::seam::proc::Deadline;
use moor_core::BoxHandle;

/// How long the runner may take to answer; a tick waits on it under the keeper's busy lock.
const RUNNER_ANSWER: Deadline = Deadline(std::time::Duration::from_secs(5));
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
    /// The runner that holds the machine's link, as its link row names it.
    pub(crate) named: &'a Runner,
    /// The same runner with its name looked up, which is dialed.
    pub(crate) dialed: &'a Runner,
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
    /// The runner it is open on, as the link row named it.
    runner: Runner,
    /// Set once the runner closed the carrier.
    ended: Arc<AtomicBool>,
}

impl Drop for MachineLink {
    fn drop(&mut self) {
        let _ = self.carrier.shutdown(Shutdown::Both);
    }
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
        let failed = |error: String| ForwardFailed { port: None, error };
        let req = Forward {
            box_id: via.box_id,
            machine_id: via.machine_id,
            ports: ports.to_vec(),
        };
        let (carrier, lines) = ask_runner(via.dialed, &req).map_err(failed)?;
        let to = ports
            .iter()
            .copied()
            .zip(tunnels.iter().map(|t| t.host_port));
        let ended = Arc::new(AtomicBool::new(false));
        let dial = DialBack {
            runner: via.dialed.clone(),
            tunnels: to.collect(),
            ended: Arc::clone(&ended),
        };
        std::thread::Builder::new()
            .name("moor-machine-forwards".to_owned())
            .spawn(move || dial.serve(lines))
            .map_err(|e| failed(e.to_string()))?;
        Ok(Self {
            tunnels,
            carrier,
            runner: via.named.clone(),
            ended,
        })
    }

    /// True while every tunnel and what serves them still run.
    pub(crate) fn alive(&self) -> bool {
        !self.ended.load(Ordering::SeqCst) && self.tunnels.iter().all(|t| t.tunnel.alive())
    }

    /// True while it is alive and open on `now`, the runner the link row names; an unknown
    /// runner keeps it, so a store hiccup cuts no forwards.
    pub(crate) fn holds(&self, now: Option<&Runner>) -> bool {
        self.alive() && now.is_none_or(|now| *now == self.runner)
    }
}

/// What answers the connections a carrier names.
struct DialBack {
    /// The runner to dial back.
    runner: Runner,
    /// The host port of each Box port's tunnel.
    tunnels: HashMap<u16, u16>,
    /// Set once the carrier ends.
    ended: Arc<AtomicBool>,
}

impl DialBack {
    /// Dials back for each connection `lines` names until the carrier ends.
    fn serve(self, mut lines: BufReader<StdConn>) {
        let mut line = String::new();
        while lines.read_line(&mut line).is_ok_and(|read| read > 0) {
            if let Ok(Carried::Conn { n, port }) = serde_json::from_str(&line) {
                if let Some(&host) = self.tunnels.get(&port) {
                    let runner = self.runner.clone();
                    std::thread::spawn(move || pipe(&runner, n, host));
                }
            }
            line.clear();
        }
        self.ended.store(true, Ordering::SeqCst);
    }
}

/// Dials `runner` back for connection `n` and copies both ways between it and host port
/// `host`, the Box port's tunnel.
fn pipe(runner: &Runner, n: u64, host: u16) {
    let Ok(back) = crate::proxy::links::connect(runner, Service::Conn(n), RUNNER_CONNECT) else {
        return;
    };
    let Ok(tunnel) = TcpStream::connect((Ipv4Addr::LOCALHOST, host)) else {
        return;
    };
    let (Ok(mut back_rx), Ok(mut tunnel_tx)) = (back.try_clone(), tunnel.try_clone()) else {
        return;
    };
    let up = std::thread::spawn(move || {
        let _ = io::copy(&mut back_rx, &mut tunnel_tx);
        let _ = tunnel_tx.shutdown(Shutdown::Write);
    });
    let (mut back, mut tunnel) = (back, tunnel);
    let _ = io::copy(&mut tunnel, &mut back);
    let _ = back.shutdown(Shutdown::Write);
    let _ = up.join();
}

/// Sends `req` to `runner`; the connection to hold while the forwards are up, and its lines.
fn ask_runner(runner: &Runner, req: &Forward) -> Result<(StdConn, BufReader<StdConn>), String> {
    let unreachable = |e: io::Error| format!("the machine runner went away: {e}");
    let mut conn = crate::proxy::links::connect(runner, Service::Forwards, RUNNER_CONNECT)?;
    conn.set_read_timeout(Some(RUNNER_ANSWER.0))
        .map_err(unreachable)?;
    let mut line = serde_json::to_vec(req).map_err(|e| e.to_string())?;
    line.push(b'\n');
    io::Write::write_all(&mut conn, &line).map_err(unreachable)?;
    let mut lines = BufReader::new(conn.try_clone().map_err(unreachable)?);
    let mut answer = String::new();
    lines.read_line(&mut answer).map_err(unreachable)?;
    match serde_json::from_str(&answer) {
        Ok(Forwarding::Up) => {
            conn.set_read_timeout(None).map_err(unreachable)?;
            Ok((conn, lines))
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
    if link.as_ref().is_some_and(|l| l.holds(now)) {
        return Ok(());
    }
    *link = None;
    *link = Some(open()?);
    Ok(())
}

#[cfg(test)]
mod tests;
