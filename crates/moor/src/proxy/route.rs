//! Which Proxy a Box's credential hosts reach, as the controller's environment sets it.

use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};

use moor_core::box_key::is_machine_handle;
use moor_core::BoxHandle;

use crate::env::{Env, EGRESS_FORWARD_BIN, EGRESS_LISTEN, EGRESS_URL};

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
        /// The private address of this host it also listens on, which a machine's Box dials as
        /// `moor-host`; `None` when no Box of a Docker machine may be served.
        host: Option<SocketAddr>,
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
                let host = env
                    .get(EGRESS_LISTEN)
                    .filter(|a| !a.is_empty())
                    .map(private_listen)
                    .transpose()?;
                env.state_dir()
                    .map(|dir| Self::Local {
                        socket: local_socket(&dir),
                        host,
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

    /// The address the forwarder of the Box at `handle`, whose tunnel listens on `box_port`,
    /// dials. A machine's Box has no tunnel: it dials the controller's Proxy as `moor-host`.
    ///
    /// # Errors
    /// A machine's Box on the local route, and no [`EGRESS_LISTEN`] address to dial.
    pub(crate) fn upstream(
        &self,
        handle: Option<&BoxHandle>,
        box_port: u16,
    ) -> Result<String, String> {
        Ok(match self {
            Self::Direct { url } => url.clone(),
            Self::Local { host, .. } if handle.is_some_and(|h| is_machine_handle(h.as_str())) => {
                let host = host.ok_or_else(|| {
                    format!(
                        "a machine's Box dials the Proxy as moor-host, so set {EGRESS_LISTEN} to a private address of this host, such as the docker0 address and a free port"
                    )
                })?;
                format!("ws://moor-host:{}{}", host.port(), super::egress::PATH)
            }
            Self::Local { .. } => {
                format!("ws://127.0.0.1:{box_port}{}", super::egress::PATH)
            }
        })
    }

    /// The socket each Box's tunnel reaches, on the local route.
    pub(crate) fn socket(&self) -> Option<&Path> {
        match self {
            Self::Local { socket, .. } => Some(socket),
            Self::Direct { .. } => None,
        }
    }
}

/// `addr` as the address the controller's Proxy also listens on. Boxes dial it over plain
/// `ws://`, so it must be a loopback or private address: never a public or wildcard one.
fn private_listen(addr: &str) -> Result<SocketAddr, String> {
    let bad = |why: &str| format!("{EGRESS_LISTEN} {addr}: {why}");
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
