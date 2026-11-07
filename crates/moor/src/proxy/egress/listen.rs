//! Where the shared egress Proxy listens.

use std::net::SocketAddr;
use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
use std::path::PathBuf;

use tokio::net::{TcpListener, UnixListener};

/// Where the Proxy listens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Listen {
    /// A TCP address.
    Tcp(SocketAddr),
    /// A Unix socket only moor's user can open, which each Box's tunnel reaches on a dev host.
    Unix(PathBuf),
}

impl std::fmt::Display for Listen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tcp(addr) => write!(f, "{addr}"),
            Self::Unix(path) => write!(f, "{}", path.display()),
        }
    }
}

/// A bound listener.
pub(super) enum Listener {
    /// On TCP.
    Tcp(TcpListener),
    /// On a Unix socket.
    Unix(UnixListener),
}

impl Listen {
    /// Binds it, replacing a stale socket file but never one a live Proxy listens on; what it is bound to, with the port a TCP
    /// address of port 0 got.
    pub(super) async fn bind(&self) -> std::io::Result<(Listener, Self)> {
        match self {
            Self::Tcp(addr) => {
                let l = TcpListener::bind(addr).await?;
                let bound = l.local_addr()?;
                Ok((Listener::Tcp(l), Self::Tcp(bound)))
            }
            Self::Unix(path) => {
                if let Some(dir) = path.parent() {
                    std::fs::DirBuilder::new()
                        .recursive(true)
                        .mode(0o700)
                        .create(dir)?;
                }
                if std::os::unix::net::UnixStream::connect(path).is_ok() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::AddrInUse,
                        "another Proxy listens on it",
                    ));
                }
                match std::fs::remove_file(path) {
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
                    _ => {}
                }
                let l = UnixListener::bind(path)?;
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
                Ok((Listener::Unix(l), self.clone()))
            }
        }
    }
}
