//! Refuses, at startup, any socket path that cannot fit a Unix socket address.

use std::path::{Path, PathBuf};

use moor_core::config::ControllerConfig;

use super::{Stop, REFUSED};
use crate::env::Env;
use crate::{front_dir, runner_dir};

/// The longest socket path in bytes: `sun_path` less its NUL, 107 on Linux and 103 on macOS.
#[cfg(target_os = "macos")]
pub(super) const LIMIT: usize = 103;
/// The longest socket path in bytes.
#[cfg(not(target_os = "macos"))]
pub(super) const LIMIT: usize = 107;

/// Refuses the first socket the controller would bind that is longer than [`LIMIT`].
pub(super) fn check(controller: &Path, cfg: &ControllerConfig, env: &Env) -> Result<(), Stop> {
    let shorter = "set `state_dir` (or MOOR_STATE_DIR) to a shorter path, such as /tmp/moor";
    let mut sockets: Vec<(PathBuf, &str)> = vec![(
        controller.to_owned(),
        if cfg.socket.is_some() {
            "set a shorter `socket`"
        } else {
            shorter
        },
    )];
    if let Some(state) = env.state_dir() {
        sockets.push((runner_dir::of(&state).join(runner_dir::SOCKET), shorter));
        for ns in cfg.fronts.keys() {
            let socket = front_dir::of(&state, ns).join(front_dir::FRONT_SOCKET);
            sockets.push((socket, shorter));
        }
    }
    for (path, advice) in sockets {
        let len = path.as_os_str().len();
        if len > LIMIT {
            return Err(Stop {
                code: REFUSED,
                message: format!(
                    "socket path {} is too long for a Unix socket: {len} bytes, the limit is {LIMIT}; {advice}",
                    path.display()
                ),
            });
        }
    }
    Ok(())
}
