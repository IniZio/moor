//! Which Proxy a Box's credential hosts reach, as the controller's environment sets it.

use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};

use crate::env::{Env, EGRESS_FORWARD_BIN, EGRESS_LISTEN, EGRESS_RELAYED, EGRESS_URL};

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
    /// The controller's own shared Proxy on `socket`, which each Box dials as `moor-host`.
    Local {
        /// The socket it listens on.
        socket: PathBuf,
        /// The private address of this host it also listens on, which a Box dials as
        /// `moor-host`; `None` when no Box may be served.
        host: Option<SocketAddr>,
    },
    /// The controller's own shared Proxy on `socket`, reached through a relay another process
    /// runs on `host`, which a Box dials as `moor-host`. The controller does not bind `host`.
    Relayed {
        /// The socket the Proxy listens on, which the relay connects to.
        socket: PathBuf,
        /// The private address the relay listens on.
        host: SocketAddr,
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
                let named = |var| env.get(var).filter(|a| !a.is_empty());
                let (listen, relayed) = (named(EGRESS_LISTEN), named(EGRESS_RELAYED));
                if listen.is_some() && relayed.is_some() {
                    return Err(format!(
                        "set only one of {EGRESS_LISTEN}, which binds the Proxy there, and {EGRESS_RELAYED}, which names a relay that already listens there"
                    ));
                }
                let socket = env
                    .state_dir()
                    .map(|dir| local_socket(&dir))
                    .ok_or_else(|| "no state directory for the shared egress Proxy".to_owned())?;
                match (listen, relayed) {
                    (_, Some(addr)) => Ok(Self::Relayed {
                        socket,
                        host: private_listen(EGRESS_RELAYED, addr)?,
                    }),
                    (listen, None) => Ok(Self::Local {
                        socket,
                        host: listen.map(|a| private_listen(EGRESS_LISTEN, a)).transpose()?,
                    }),
                }
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

    /// The address each Box's forwarder dials. A Box on the controller's own Proxy dials it as
    /// `moor-host`.
    ///
    /// # Errors
    /// The local route and no [`EGRESS_LISTEN`] or [`EGRESS_RELAYED`] address to dial.
    pub(crate) fn upstream(&self) -> Result<String, String> {
        Ok(match self {
            Self::Direct { url } => url.clone(),
            Self::Local {
                host: Some(host), ..
            }
            | Self::Relayed { host, .. } => {
                format!("ws://moor-host:{}{}", host.port(), super::egress::PATH)
            }
            Self::Local { host: None, .. } => {
                return Err(format!(
                    "a Box dials the Proxy as moor-host, so set {EGRESS_LISTEN} to a private address of this host, such as the docker0 address and a free port"
                ));
            }
        })
    }

    /// The socket the controller's own Proxy listens on, on the local route.
    pub(crate) fn socket(&self) -> Option<&Path> {
        match self {
            Self::Local { socket, .. } | Self::Relayed { socket, .. } => Some(socket),
            Self::Direct { .. } => None,
        }
    }

    /// The address the controller's own Proxy binds besides its socket. A relayed address is
    /// the relay's, so the controller leaves it alone.
    pub(crate) const fn bind(&self) -> Option<SocketAddr> {
        match self {
            Self::Local { host, .. } => *host,
            Self::Relayed { .. } | Self::Direct { .. } => None,
        }
    }
}

/// `addr`, the value of `var`, as the address a Proxy also listens on. Boxes dial it over plain
/// `ws://`, so it must be a loopback or private address: never a public or wildcard one.
fn private_listen(var: &str, addr: &str) -> Result<SocketAddr, String> {
    let bad = |why: &str| format!("{var} {addr}: {why}");
    let parsed: SocketAddr = addr.parse().map_err(|_| bad("not an IP:PORT address"))?;
    let private = match parsed.ip() {
        IpAddr::V4(ip) => {
            ip.is_loopback()
                || ip.is_private()
                || ip.is_link_local()
                // Carrier-grade NAT space, where tailscale puts its addresses.
                || (ip.octets()[0] == 100 && (64..128).contains(&ip.octets()[1]))
        }
        IpAddr::V6(ip) => ip.is_loopback() || (ip.segments()[0] & 0xfe00) == 0xfc00,
    };
    if private && parsed.port() != 0 {
        Ok(parsed)
    } else {
        Err(bad(
            "plain ws:// is only for a loopback or private address with a port; serve the Proxy over wss:// and set MOOR_EGRESS_URL instead",
        ))
    }
}

/// Whether the shared egress Proxy runs on this host, for the controller `env` sets up.
pub(crate) fn is_local(env: &Env) -> bool {
    EgressRoute::from_env(env).is_ok_and(|r| r.socket().is_some())
}

/// The socket the controller's own shared Proxy listens on.
pub(crate) fn local_socket(state_dir: &Path) -> PathBuf {
    state_dir.join(LOCAL_SOCKET)
}

/// The key the shared egress Proxy checks token MACs with; empty when it is off.
pub(crate) fn key(env: &Env) -> &[u8] {
    env.egress_key()
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
