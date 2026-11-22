//! Which Proxy a Box's credential hosts reach, as the controller's environment sets it.

use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};

use crate::env::{Env, EGRESS_FORWARD_BIN, EGRESS_RELAYED, EGRESS_URL};

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
    /// The controller's own shared Proxy on `socket`, which no Box can reach without a relay.
    Local {
        /// The socket it listens on.
        socket: PathBuf,
    },
    /// The controller's own shared Proxy on `socket`, reached through a relay another process
    /// runs on `host`, which a Box dials. The controller does not bind `host`.
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
                let socket = env
                    .state_dir()
                    .map(|dir| local_socket(&dir))
                    .ok_or_else(|| "no state directory for the shared egress Proxy".to_owned())?;
                match env.get(EGRESS_RELAYED).filter(|a| !a.is_empty()) {
                    Some(addr) => Ok(Self::Relayed {
                        socket,
                        host: private_relay(addr)?,
                    }),
                    None => Ok(Self::Local { socket }),
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

    /// The address each Box's forwarder dials. On a relayed Proxy it is the relay's own address,
    /// which a nested Docker's networks never cover.
    ///
    /// # Errors
    /// The local route, which has no [`EGRESS_RELAYED`] address to dial.
    pub(crate) fn upstream(&self) -> Result<String, String> {
        match self {
            Self::Direct { url } => Ok(url.clone()),
            Self::Relayed { host, .. } => Ok(format!("ws://{host}{}", super::egress::PATH)),
            Self::Local { .. } => Err(format!(
                "a Box reaches the Proxy only through a relay, so set {EGRESS_RELAYED} to the relay's address, as moor up does"
            )),
        }
    }

    /// The socket the controller's own Proxy listens on, on the local route.
    pub(crate) fn socket(&self) -> Option<&Path> {
        match self {
            Self::Local { socket, .. } | Self::Relayed { socket, .. } => Some(socket),
            Self::Direct { .. } => None,
        }
    }
}

/// `addr`, the value of [`EGRESS_RELAYED`], as the address a relay listens on. Boxes dial it over
/// plain `ws://`, so it must be a loopback or private address: never a public or wildcard one.
fn private_relay(addr: &str) -> Result<SocketAddr, String> {
    let bad = |why: &str| format!("{EGRESS_RELAYED} {addr}: {why}");
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
