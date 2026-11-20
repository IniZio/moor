//! Refuses, at startup, any socket path that cannot fit a Unix socket address.

use std::path::{Path, PathBuf};

use moor_core::config::ControllerConfig;

use super::{Stop, REFUSED};
use crate::env::Env;
use crate::{front_dir, ledger_dir, runner_dir};

/// The longest socket path in bytes: `sun_path` less its NUL, 107 on Linux and 103 on macOS.
#[cfg(target_os = "macos")]
pub(crate) const LIMIT: usize = 103;
/// The longest socket path in bytes.
#[cfg(not(target_os = "macos"))]
pub(crate) const LIMIT: usize = 107;

/// What to change when a socket under the state dir is too long.
pub(crate) const SHORTER: &str =
    "set `state_dir` (or MOOR_STATE_DIR) to a shorter path, such as /tmp/moor";

/// A socket path to fit, and whether it is bound first as a staged name one byte longer.
pub(crate) type Socket = (PathBuf, bool);

/// The sockets the controller binds under `state`. The shared egress Proxy's socket is as long
/// as the runner's and bound under its own name, so it fits whenever the runner's does.
pub(crate) fn under_state(state: &Path, cfg: &ControllerConfig) -> Vec<Socket> {
    let mut sockets = vec![
        (runner_dir::of(state).join(runner_dir::SOCKET), true),
        (ledger_dir::socket(state), true),
    ];
    for ns in cfg.fronts.keys() {
        sockets.push((front_dir::of(state, ns).join(front_dir::FRONT_SOCKET), true));
    }
    sockets
}

/// Why `path` cannot be bound as a Unix socket, ending with `advice`; none when it fits.
pub(crate) fn too_long((path, staged): &Socket, advice: &str) -> Option<String> {
    let limit = LIMIT - usize::from(*staged);
    let len = path.as_os_str().len();
    (len > limit).then(|| {
        format!(
            "socket path {} is too long for a Unix socket: {len} bytes, the limit is {limit}; {advice}",
            path.display()
        )
    })
}

/// Refuses the first socket the controller would bind that does not fit.
pub(super) fn check(controller: &Path, cfg: &ControllerConfig, env: &Env) -> Result<(), Stop> {
    let mut sockets: Vec<(Socket, &str)> = vec![(
        (controller.to_owned(), true),
        if cfg.socket.is_some() {
            "set a shorter `socket`"
        } else {
            SHORTER
        },
    )];
    if let Some(state) = env.state_dir() {
        sockets.extend(under_state(&state, cfg).into_iter().map(|s| (s, SHORTER)));
    }
    sockets
        .iter()
        .find_map(|(s, advice)| too_long(s, advice))
        .map_or(Ok(()), |message| {
            Err(Stop {
                code: REFUSED,
                message,
            })
        })
}
