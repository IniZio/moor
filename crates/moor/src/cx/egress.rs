//! Which Proxy a Box's credential hosts reach, as the controller's environment sets it.

use std::net::SocketAddr;
use std::path::PathBuf;

use crate::env::{Env, EGRESS_FORWARD_BIN, EGRESS_KEY, EGRESS_URL};

/// Where a Box's credential hosts go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EgressRoute {
    /// The Box's own Proxy, over its tunnel.
    PerBox,
    /// The shared Proxy at `url`, which the Box dials itself.
    Direct {
        /// The `ws://` or `wss://` address of its `/egress`.
        url: String,
    },
    /// The shared Proxy on the controller's loopback at `relay`, which the Box reaches over its
    /// tunnel; the tunnel's front relays each connection there.
    Tunneled {
        /// The loopback address it listens on.
        relay: SocketAddr,
        /// The path of its `/egress`.
        path: String,
    },
}

impl EgressRoute {
    /// The route the controller's environment sets. The shared Proxy serves Boxes only when
    /// `MOOR_EGRESS_KEY` is set, and then `MOOR_EGRESS_URL` names it.
    ///
    /// # Errors
    /// The key is set and the address is missing or not a `ws://` or `wss://` address.
    pub(crate) fn from_env(env: &Env) -> Result<Self, String> {
        if env.get(EGRESS_KEY).is_none_or(str::is_empty) {
            return Ok(Self::PerBox);
        }
        let url = env
            .get(EGRESS_URL)
            .filter(|u| !u.is_empty())
            .ok_or("MOOR_EGRESS_URL is required when MOOR_EGRESS_KEY is set")?;
        Self::parse(url)
    }

    /// The route to the shared Proxy at `url`: `wss://` directly, or `ws://` to a loopback
    /// address over each Box's tunnel, since only the controller's host can reach that.
    ///
    /// # Errors
    /// As for [`Self::from_env`].
    pub(crate) fn parse(url: &str) -> Result<Self, String> {
        let bad = || format!("MOOR_EGRESS_URL {url} is not a ws:// or wss:// address");
        let (plain, rest) = match (url.strip_prefix("ws://"), url.strip_prefix("wss://")) {
            (Some(rest), _) => (true, rest),
            (_, Some(rest)) => (false, rest),
            _ => return Err(bad()),
        };
        let (authority, path) = rest.find('/').map_or((rest, "/"), |i| rest.split_at(i));
        if authority.is_empty() {
            return Err(bad());
        }
        if !plain {
            return Ok(Self::Direct {
                url: url.to_owned(),
            });
        }
        match authority.parse::<SocketAddr>() {
            Ok(relay) if relay.ip().is_loopback() => Ok(Self::Tunneled {
                relay,
                path: path.to_owned(),
            }),
            _ => Err(format!(
                "MOOR_EGRESS_URL {url} would send Box tokens in the clear; use wss://, or ws:// only to a loopback address"
            )),
        }
    }

    /// The address the forwarder of a Box whose tunnel listens on `box_port` dials; none on the
    /// per-Box route.
    pub(crate) fn upstream(&self, box_port: u16) -> Option<String> {
        match self {
            Self::PerBox => None,
            Self::Direct { url } => Some(url.clone()),
            Self::Tunneled { path, .. } => Some(format!("ws://127.0.0.1:{box_port}{path}")),
        }
    }

    /// Where the tunnel's front relays each connection, on the tunneled route.
    pub(crate) const fn relay(&self) -> Option<SocketAddr> {
        match self {
            Self::Tunneled { relay, .. } => Some(*relay),
            Self::PerBox | Self::Direct { .. } => None,
        }
    }
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
