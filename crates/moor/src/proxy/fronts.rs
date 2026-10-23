//! The front Proxy: one listener per front whose controller config lists `proxy_rules`. A
//! front reaches it only through its shim, with a lease as the proxy password.

use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use super::front::FrontSocket;

/// The socket of front `ns`'s Proxy.
pub(crate) fn socket_path(sockets: &Path, state_dir: &Path, ns: &str) -> PathBuf {
    FrontSocket::of_front(sockets, 0, state_dir, ns)
        .path()
        .to_owned()
}

/// Whether front `ns`'s Proxy accepts connections.
pub(crate) fn running(sockets: &Path, state_dir: &Path, ns: &str) -> bool {
    UnixStream::connect(socket_path(sockets, state_dir, ns)).is_ok()
}
