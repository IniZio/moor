//! The machine runner's directory, `<state>/runner/`: its socket to the controller and the
//! socket it takes port forwards on, so the runner can be handed that one directory and not
//! the controller's own socket.

use std::path::{Path, PathBuf};

/// The runner's socket to the controller.
pub(crate) const SOCKET: &str = "runner.sock";

/// `<state>/runner`.
pub(crate) fn of(state_dir: &Path) -> PathBuf {
    state_dir.join("runner")
}
