//! Exposes a host port inside a Box through `crabbox tunnel --reverse`.
//!
//! crabbox keeps running after it reports the tunnel ready, so the call is a
//! long-lived child; the returned handle owns it and dropping the handle kills it.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use moor_core::name::BoxHandle;
use moor_core::seam::driver::{DriverError, TunnelFailure, TunnelHandle};
use moor_core::seam::proc::{ChildHandle, Deadline, FirstLineError, Proc};
use serde::Deserialize;

use super::{static_env, target_args, Crabbox};

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
    /// Port on the host.
    host: u16,
}

/// A running tunnel child.
struct Tunnel {
    /// The crabbox process; dropping it kills its process group.
    child: Box<dyn ChildHandle>,
    /// Spawner, asked whether the child still lives.
    proc: Arc<dyn Proc>,
}

impl TunnelHandle for Tunnel {
    fn pid(&self) -> u32 {
        self.child.pid()
    }

    fn alive(&self) -> bool {
        self.proc.alive(self.child.pid())
    }
}

/// Starts a tunnel that serves host `127.0.0.1:host_port` as Box `127.0.0.1:box_port`.
pub(super) fn reverse_tunnel(
    cb: &Crabbox,
    handle: &BoxHandle,
    box_port: u16,
    host_port: u16,
    host_cwd: &Path,
) -> Result<Box<dyn TunnelHandle>, DriverError> {
    let spec = static_env(
        Crabbox::spec(TUNNEL_READY_DEADLINE)
            .arg("tunnel")
            .args(target_args(handle))
            .arg("--reverse")
            .arg(format!("{box_port}:{host_port}"))
            .arg("--json")
            .cwd(host_cwd),
        handle,
    );
    let mut child = cb
        .proc
        .spawn_child(&spec)
        .map_err(|_| DriverError::Unavailable("crabbox could not be started".to_owned()))?;
    match child.first_line(Deadline(TUNNEL_READY_DEADLINE)) {
        Ok(line) => match serde_json::from_str::<Ready>(&line) {
            Ok(r) if r.r#box == box_port && r.host == host_port => Ok(Box::new(Tunnel {
                child,
                proc: Arc::clone(&cb.proc),
            })),
            _ => Err(DriverError::Failed {
                summary: UNEXPECTED_READY.to_owned(),
            }),
        },
        Err(FirstLineError::Exited {
            code,
            signal,
            stderr_tail,
        }) => Err(DriverError::Tunnel {
            failure: TunnelFailure::Exited { code, signal },
            stderr_tail,
        }),
        Err(FirstLineError::Timeout { after, .. }) => Err(DriverError::Tunnel {
            failure: TunnelFailure::TimedOut(after),
            stderr_tail: String::new(),
        }),
        Err(FirstLineError::Io(_)) => Err(DriverError::Failed {
            summary: "could not read the reverse tunnel's output".to_owned(),
        }),
    }
}

#[cfg(test)]
mod tests;
