//! Which Proxy a Box's credential hosts reach, as the controller's environment sets it.

use std::path::{Path, PathBuf};

use crate::env::{Env, EGRESS_FORWARD_BIN, EGRESS_KEY, EGRESS_URL};

/// The controller's own shared egress Proxy socket, under the state dir.
const LOCAL_SOCKET: &str = "egress/egress.sock";

/// Where a Box's credential hosts go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EgressRoute {
    /// The shared Proxy at `url`, which each Box dials itself.
    Direct {
        /// The `wss://` address of its `/egress`.
        url: String,
    },
    /// The controller's own shared Proxy on `socket`, which each Box reaches over its tunnel.
    Local {
        /// The socket it listens on.
        socket: PathBuf,
    },
}

impl EgressRoute {
    /// The route the controller's environment sets: the shared Proxy `MOOR_EGRESS_URL` names,
    /// else the controller's own under `state_dir`.
    ///
    /// # Errors
    /// The address is not a `wss://` address, or neither it nor a state dir is set.
    pub(crate) fn from_env(env: &Env) -> Result<Self, String> {
        env.get(EGRESS_URL).filter(|u| !u.is_empty()).map_or_else(
            || {
                env.state_dir()
                    .map(|dir| Self::Local {
                        socket: local_socket(&dir),
                    })
                    .ok_or_else(|| "no state directory for the shared egress Proxy".to_owned())
            },
            Self::parse,
        )
    }

    /// The route to the shared Proxy at `url`.
    ///
    /// # Errors
    /// `url` is not a `wss://` address with a host.
    pub(crate) fn parse(url: &str) -> Result<Self, String> {
        let host = url
            .strip_prefix("wss://")
            .and_then(|rest| rest.split('/').next())
            .filter(|h| !h.is_empty());
        match host {
            Some(_) => Ok(Self::Direct {
                url: url.to_owned(),
            }),
            None => Err(format!(
                "MOOR_EGRESS_URL {url} is not a wss:// address; unset it to run the shared Proxy on this host"
            )),
        }
    }

    /// The address the forwarder of a Box whose tunnel listens on `box_port` dials.
    pub(crate) fn upstream(&self, box_port: u16) -> String {
        match self {
            Self::Direct { url } => url.clone(),
            Self::Local { .. } => {
                format!("ws://127.0.0.1:{box_port}{}", super::egress::PATH)
            }
        }
    }

    /// The socket each Box's tunnel reaches, on the local route.
    pub(crate) fn socket(&self) -> Option<&Path> {
        match self {
            Self::Local { socket } => Some(socket),
            Self::Direct { .. } => None,
        }
    }
}

/// The socket the controller's own shared Proxy listens on.
pub(crate) fn local_socket(state_dir: &Path) -> PathBuf {
    state_dir.join(LOCAL_SOCKET)
}

/// The key the shared egress Proxy checks token MACs with; empty when it is off.
pub(crate) fn key(env: &Env) -> &[u8] {
    env.get(EGRESS_KEY).unwrap_or_default().as_bytes()
}

/// The forwarder binary the controller delivers: `MOOR_EGRESS_FORWARD_BIN`, else
/// `moor-egress-forward` beside the running `moor`.
pub(crate) fn binary(env: &Env) -> PathBuf {
    if let Some(path) = env.get(EGRESS_FORWARD_BIN).filter(|p| !p.is_empty()) {
        return PathBuf::from(path);
    }
    std::env::current_exe().map_or_else(
        |_| PathBuf::from("moor-egress-forward"),
        |exe| exe.with_file_name("moor-egress-forward"),
    )
}
