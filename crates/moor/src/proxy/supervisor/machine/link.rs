//! A user's machine attached to a Box: which machine, and the forwards that let it reach the
//! Box's ports at the same numbers on its own loopback.
//!
//! Each Box port gets a driver local tunnel to a free host loopback port; one ssh child to the
//! machine then serves every one of them back as the Box port on the machine's loopback.

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

/// The running forwards of one attached machine; dropping it ends every child.
pub(crate) struct MachineLink {
    /// One tunnel per Box port.
    tunnels: Vec<LocalTunnel>,
    /// The ssh child that serves the tunnels on the machine.
    ssh: Box<dyn ChildHandle>,
}

impl MachineLink {
    /// Opens a tunnel per port in `ports`, then the one ssh child to `machine`.
    ///
    /// # Errors
    /// The first port whose tunnel did not open, or the ssh child that did not start.
    pub(crate) fn open(
        driver: &dyn Driver,
        proc: &dyn Proc,
        handle: &BoxHandle,
        machine: &UserMachine,
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
        let spec = ProcSpec::new(Tool::Ssh, SSH_START).args(forward_args(machine, &forwards));
        let ssh = proc.spawn_child(&spec).map_err(|e| ForwardFailed {
            port: None,
            error: e.to_string(),
        })?;
        Ok(Self { tunnels, ssh })
    }

    /// True while every tunnel and the ssh child still run.
    pub(crate) fn alive(&mut self) -> bool {
        self.ssh.try_exit().is_none() && self.tunnels.iter().all(|t| t.tunnel.alive())
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
