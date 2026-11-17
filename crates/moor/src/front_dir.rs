//! Each front's own directory, `<state>/fronts/<ns>/`. It holds the front's controller socket
//! and nothing else, so a front the controller does not start can be handed that one directory.

use std::path::{Path, PathBuf};

/// The front's socket to the controller. The names stay short because the whole path must fit
/// a Unix socket address.
pub(crate) const FRONT_SOCKET: &str = "front";

/// `<state>/fronts`, which holds every front's directory and the pid files of started fronts.
pub(crate) fn root(state_dir: &Path) -> PathBuf {
    state_dir.join("fronts")
}

/// The directory of front `ns`.
pub(crate) fn of(state_dir: &Path, ns: &str) -> PathBuf {
    root(state_dir).join(ns)
}
