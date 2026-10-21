//! Whom a connection speaks for: the controller socket's peer uid, or the front whose socket
//! it arrived on.

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::net::{UnixListener, UnixStream};

use moor_core::user::Caller;

use super::{bind, mkdir, Stop, USAGE};
use crate::env::Env;

/// Binds `<state>/fronts/<namespace>.sock` for each front: each socket 0600 in a 0700 dir.
pub(super) fn bind_fronts(
    env: &Env,
    fronts: &[&String],
) -> Result<Vec<(String, UnixListener)>, Stop> {
    if fronts.is_empty() {
        return Ok(Vec::new());
    }
    let failed = |message: String| Stop { code: 1, message };
    let dir = env
        .state_dir()
        .map(|d| d.join("fronts"))
        .ok_or_else(|| Stop {
            code: USAGE,
            message: "fronts need a state directory: set state_dir or MOOR_STATE_DIR".into(),
        })?;
    mkdir(&dir)
        .and_then(|()| fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)))
        .map_err(|e| failed(format!("cannot prepare {}: {e}", dir.display())))?;
    fronts
        .iter()
        .map(|ns| {
            let socket = dir.join(format!("{ns}.sock"));
            bind(&socket)
                .map(|listener| ((*ns).clone(), listener))
                .map_err(|e| failed(format!("cannot bind {}: {e}", socket.display())))
        })
        .collect()
}

/// Whom a connection on the controller socket speaks for, from the peer's uid: the operator
/// when it is the controller's own uid.
pub(super) fn local_caller(stream: &UnixStream) -> Result<Caller, String> {
    let own = rustix::process::geteuid().as_raw();
    peer_uid(stream)
        .map(|uid| {
            if uid == own {
                Caller::Operator(uid)
            } else {
                Caller::Local(uid)
            }
        })
        .map_err(|e| format!("cannot read the peer's uid: {e}"))
}

/// The uid of the process at the other end of `stream`.
#[cfg(target_os = "linux")]
fn peer_uid(stream: &UnixStream) -> io::Result<u32> {
    Ok(rustix::net::sockopt::socket_peercred(stream)?.uid.as_raw())
}

/// The uid of the process at the other end of `stream`; read only on Linux.
#[cfg(not(target_os = "linux"))]
fn peer_uid(_stream: &UnixStream) -> io::Result<u32> {
    Err(io::ErrorKind::Unsupported.into())
}
