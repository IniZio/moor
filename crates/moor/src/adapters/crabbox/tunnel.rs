//! Exposes a host Unix socket inside a Box through `crabbox tunnel --reverse`.
//!
//! crabbox keeps running after it reports the tunnel ready, so the call is a
//! long-lived child; the returned handle owns it and dropping the handle kills it.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use moor_core::name::BoxHandle;
use moor_core::seam::driver::{DriverError, LocalTunnel, TunnelFailure, TunnelHandle};
use moor_core::seam::proc::{ChildHandle, Deadline, FirstLineError, ProcSpec};
use serde::Deserialize;

use super::{target_args, Crabbox};

/// Time allowed for the tunnel to report ready. crabbox gives up on its own after
/// 15 s with a message on stderr; this is longer so that message normally wins.
const TUNNEL_READY_DEADLINE: Duration = Duration::from_secs(30);

/// Failure summary when the ready line is not the one requested.
const UNEXPECTED_READY: &str = "reverse tunnel printed an unexpected ready line";

/// The one stdout line crabbox prints once the Box-side port accepts.
#[derive(Deserialize)]
struct Ready {
    /// Port inside the Box.
    r#box: u16,
    /// Socket on the host.
    socket: PathBuf,
}

/// A running tunnel child.
struct Tunnel {
    /// The crabbox process; dropping it kills its process group. Behind a lock
    /// because `alive` takes `&self` while the exit check needs `&mut`.
    child: Mutex<Box<dyn ChildHandle>>,
}

impl TunnelHandle for Tunnel {
    fn pid(&self) -> u32 {
        self.child
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pid()
    }

    /// Asks the child handle's own exit state, never the pid, which can be reused.
    fn alive(&self) -> bool {
        self.child
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .try_exit()
            .is_none()
    }
}

/// Starts a tunnel that serves the host Unix socket `socket` as Box `127.0.0.1:box_port`.
pub(super) fn reverse_tunnel(
    cb: &Crabbox,
    handle: &BoxHandle,
    box_port: u16,
    socket: &Path,
) -> Result<Box<dyn TunnelHandle>, DriverError> {
    if super::pool::is_pool(handle) {
        return Err(DriverError::Unsupported("a pool Box has no tunnel yet"));
    }
    let mut forward = OsString::from(format!("{box_port}:"));
    forward.push(socket);
    let spec = cb.at_lease(
        Crabbox::spec(TUNNEL_READY_DEADLINE)
            .arg("tunnel")
            .args(target_args(handle))
            .arg("--reverse")
            .arg(forward)
            .arg("--json"),
        handle,
    )?;
    cb.retry_claim_race(|| open(cb, &spec, box_port, socket))
}

/// Starts a tunnel that forwards a host loopback port crabbox picks to Box `127.0.0.1:box_port`.
/// crabbox prints `http://127.0.0.1:<port>` once the host port accepts.
pub(super) fn local_tunnel(
    cb: &Crabbox,
    handle: &BoxHandle,
    box_port: u16,
) -> Result<LocalTunnel, DriverError> {
    if super::pool::is_pool(handle) {
        return Err(DriverError::Unsupported("a pool Box has no tunnel yet"));
    }
    let spec = cb.at_lease(
        Crabbox::spec(TUNNEL_READY_DEADLINE)
            .arg("tunnel")
            .args(target_args(handle))
            .arg("--local-port")
            .arg("0")
            .arg(box_port.to_string()),
        handle,
    )?;
    cb.retry_claim_race(|| {
        let mut child = cb
            .proc
            .spawn_child(&spec)
            .map_err(|_| DriverError::Unavailable("crabbox could not be started".to_owned()))?;
        let line = child
            .first_line(Deadline(TUNNEL_READY_DEADLINE))
            .map_err(ready_error)?;
        let host_port = line
            .trim()
            .strip_prefix("http://127.0.0.1:")
            .and_then(|p| p.parse::<u16>().ok())
            .ok_or_else(|| DriverError::Failed {
                summary: "local tunnel printed an unexpected ready line".to_owned(),
            })?;
        Ok(LocalTunnel {
            host_port,
            tunnel: Box::new(Tunnel {
                child: Mutex::new(child),
            }),
        })
    })
}

/// Spawns the tunnel child once and waits for its ready line.
fn open(
    cb: &Crabbox,
    spec: &ProcSpec,
    box_port: u16,
    socket: &Path,
) -> Result<Box<dyn TunnelHandle>, DriverError> {
    let mut child = cb
        .proc
        .spawn_child(spec)
        .map_err(|_| DriverError::Unavailable("crabbox could not be started".to_owned()))?;
    match child.first_line(Deadline(TUNNEL_READY_DEADLINE)) {
        Ok(line) => match serde_json::from_str::<Ready>(&line) {
            Ok(r) if r.r#box == box_port && r.socket == socket => Ok(Box::new(Tunnel {
                child: Mutex::new(child),
            })),
            _ => Err(DriverError::Failed {
                summary: UNEXPECTED_READY.to_owned(),
            }),
        },
        Err(e) => Err(ready_error(e)),
    }
}

/// The error for a tunnel child that did not print its ready line.
fn ready_error(e: FirstLineError) -> DriverError {
    match e {
        FirstLineError::Exited {
            code,
            signal,
            stderr_tail,
        } => DriverError::Tunnel {
            failure: TunnelFailure::Exited { code, signal },
            stderr_tail,
        },
        FirstLineError::Timeout { after, .. } => DriverError::Tunnel {
            failure: TunnelFailure::TimedOut(after),
            stderr_tail: String::new(),
        },
        FirstLineError::Io(_) => DriverError::Failed {
            summary: "could not read the tunnel's output".to_owned(),
        },
    }
}

#[cfg(test)]
mod tests;
