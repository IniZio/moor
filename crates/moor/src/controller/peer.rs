//! Whom a connection speaks for: the controller socket's peer uid, or the front whose socket
//! it arrived on.

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::net::{UnixListener, UnixStream};

use moor_core::user::Caller;

use super::inherit::Sockets;
use super::{mkdir, Stop, USAGE};
use crate::env::Env;

/// Listens on `<state>/fronts/<namespace>.sock` for each front, taking the socket passed in as
/// `front-<namespace>` when sockets were passed: each socket 0600 in a 0700 dir.
pub(super) fn bind_fronts(
    env: &Env,
    sockets: &mut Sockets,
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
            sockets
                .listen(&format!("front-{ns}"), &socket)
                .map(|listener| ((*ns).clone(), listener))
                .map_err(failed)
        })
        .collect()
}

/// Whom a connection on the controller socket speaks for, from the peer's uid: the operator
/// when it is `own`, the controller's own uid.
pub(super) fn local_caller(stream: &UnixStream, own: u32) -> Result<Caller, String> {
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

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixStream;

    use moor_core::user::Caller;

    use super::local_caller;

    #[test]
    fn a_peer_with_another_uid_is_not_the_operator() {
        let (stream, _other) = UnixStream::pair().expect("socket pair");
        let me = rustix::process::geteuid().as_raw();
        let controller = me.wrapping_add(1);
        assert_eq!(local_caller(&stream, controller), Ok(Caller::Local(me)));
        assert_eq!(local_caller(&stream, me), Ok(Caller::Operator(me)));
    }

    #[test]
    fn a_peer_with_a_uid_above_the_controllers_is_not_the_operator() {
        let (stream, _other) = UnixStream::pair().expect("socket pair");
        let me = rustix::process::geteuid().as_raw();
        let controller = me.checked_sub(1).expect("tests run as a uid above 0");
        assert_eq!(local_caller(&stream, controller), Ok(Caller::Local(me)));
    }
}
