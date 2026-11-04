//! The ledger's directory, `<state>/ledger/`: the socket Proxy processes reach the store
//! through, the controller's only, so they never hold the database URL or the vault key.

use std::path::{Path, PathBuf};

/// The ledger socket.
pub(crate) const SOCKET: &str = "proxy.sock";

/// `<state>/ledger`.
pub(crate) fn of(state_dir: &Path) -> PathBuf {
    state_dir.join("ledger")
}

/// `<state>/ledger/proxy.sock`.
pub(crate) fn socket(state_dir: &Path) -> PathBuf {
    of(state_dir).join(SOCKET)
}
