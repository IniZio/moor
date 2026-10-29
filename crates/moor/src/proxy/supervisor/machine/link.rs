//! A user's machine attached to a Box: which machine, and the forwards that let it reach the
//! Box's ports at the same numbers on its own loopback.
//!
//! Each Box port gets a driver local tunnel to a free host loopback port. One ssh child to the
//! machine, or for an enrolled machine one held connection to the machine runner, then serves
//! every one of them back as the Box port on the machine's loopback.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;

use moor_client::forward::{Forward, Forwarding};
use moor_core::config::UserMachine;
use moor_core::seam::driver::{Driver, LocalTunnel};
use moor_core::seam::proc::{ChildHandle, Deadline, Proc, ProcSpec, Tool};
use moor_core::BoxHandle;

/// How long ssh may take to start; it then runs until dropped.
const SSH_START: Deadline = Deadline(std::time::Duration::from_secs(30));

/// The ssh arguments that serve each `(box_port, host_port)` as `box_port` on the machine's
/// loopback. Only the machine's checked ssh target and port numbers reach the argv.
pub(crate) fn forward_args(machine: &UserMachine, forwards: &[(u16, u16)]) -> Vec<String> {
    let (dest, port) = destination(&machine.ssh);
    let mut args: Vec<String> = [
        "-N",
        "-o",
        "ExitOnForwardFailure=yes",
        "-o",
        "BatchMode=yes",
    ]
    .map(str::to_owned)
    .into();
    for (box_port, host_port) in forwards {
        args.push("-R".to_owned());
        args.push(format!("{box_port}:127.0.0.1:{host_port}"));
    }
    if let Some(port) = port {
        args.push("-p".to_owned());
        args.push(port.to_owned());
    }
    args.push("--".to_owned());
    args.push(dest);
    args
}

/// Splits a checked `user@host[:port]` into ssh's `user@host` and port; an IPv6 host loses
/// its brackets.
fn destination(target: &str) -> (String, Option<&str>) {
    let (user, rest) = target.split_once('@').unwrap_or(("", target));
    let (host, port) = rest.strip_prefix('[').map_or_else(
        || {
            rest.split_once(':')
                .map_or((rest, None), |(h, p)| (h, Some(p)))
        },
        |v6| {
            v6.split_once(']')
                .map_or((v6, None), |(h, tail)| (h, tail.strip_prefix(':')))
        },
    );
    (format!("{user}@{host}"), port)
}

/// Why a machine's forwards are down.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ForwardFailed {
    /// The Box port whose tunnel failed; none when the ssh child failed.
    pub(crate) port: Option<u16>,
    /// What went wrong.
    pub(crate) error: String,
}

/// How the machine is reached.
pub(crate) enum Via<'a> {
    /// The host dials its ssh target.
    Ssh(&'a UserMachine),
    /// Its agent links in to the machine runner listening on `socket`.
    Runner {
        /// The runner's forward socket.
        socket: &'a Path,
        /// The Box whose ports these are.
        box_id: i64,
        /// The machine's enrolled id.
        machine_id: i64,
    },
}

/// What serves the tunnels on the machine.
enum Carrier {
    /// The ssh child.
    Ssh(Box<dyn ChildHandle>),
    /// The connection the runner keeps the agent's listeners up for.
    Runner(UnixStream),
}

/// The running forwards of one attached machine; dropping it ends every child.
pub(crate) struct MachineLink {
    /// One tunnel per Box port.
    tunnels: Vec<LocalTunnel>,
    /// What serves the tunnels on the machine.
    carrier: Carrier,
}

impl MachineLink {
    /// Opens a tunnel per port in `ports`, then what serves them on the machine `via`.
    ///
    /// # Errors
    /// The first port whose tunnel did not open, or the ssh child or runner that did not start
    /// the forwards.
    pub(crate) fn open(
        driver: &dyn Driver,
        proc: &dyn Proc,
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
        let carrier = match *via {
            Via::Ssh(machine) => {
                let spec =
                    ProcSpec::new(Tool::Ssh, SSH_START).args(forward_args(machine, &forwards));
                Carrier::Ssh(proc.spawn_child(&spec).map_err(|e| failed(e.to_string()))?)
            }
            Via::Runner {
                socket,
                box_id,
                machine_id,
            } => {
                let req = Forward {
                    box_id,
                    machine_id,
                    ports: forwards,
                };
                Carrier::Runner(ask_runner(socket, &req).map_err(failed)?)
            }
        };
        Ok(Self { tunnels, carrier })
    }

    /// True while every tunnel and what serves them still run.
    pub(crate) fn alive(&mut self) -> bool {
        let carried = match &mut self.carrier {
            Carrier::Ssh(ssh) => ssh.try_exit().is_none(),
            Carrier::Runner(conn) => {
                matches!(conn.read(&mut [0u8; 1]), Err(e) if e.kind() == io::ErrorKind::WouldBlock)
            }
        };
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
    conn.set_read_timeout(Some(SSH_START.0))
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
