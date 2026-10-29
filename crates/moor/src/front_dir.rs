//! Each front's own directory, `<state>/fronts/<ns>/`. It holds the front's controller socket
//! and, when the front has a Proxy, that Proxy's socket and the CA certificate it signs with,
//! and nothing else, so a front the controller does not start can be handed that one directory.

use std::path::{Path, PathBuf};

/// The front's socket to the controller. The names stay short because the whole path must fit
/// a Unix socket address.
pub(crate) const FRONT_SOCKET: &str = "front";
/// The socket of the front's Proxy.
pub(crate) const PROXY_SOCKET: &str = "proxy";
/// The certificate, in PEM form, of the CA the front's Proxy signs with.
pub(crate) const CA_PEM: &str = "ca.pem";

/// `<state>/fronts`, which holds every front's directory and the pid files of started fronts.
pub(crate) fn root(state_dir: &Path) -> PathBuf {
    state_dir.join("fronts")
}

/// The directory of front `ns`.
pub(crate) fn of(state_dir: &Path, ns: &str) -> PathBuf {
    root(state_dir).join(ns)
}
