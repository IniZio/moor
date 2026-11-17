//! The hosts a Box reaches through its Proxy that the Proxy serves itself: `machines.service`,
//! the machine runner's tools for the machine a Box holds, and `moor_service`, the gate's
//! Box-facing listener where the agent reports workflow steps. Each is one entry, so the route
//! and the MCP server a Box gets cannot disagree.

use std::net::IpAddr;

use serde::{Deserialize, Deserializer};

use super::McpServer;
use crate::config::{ConfigError, SyncConfig};

/// `machines:` section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct MachinesConfig {
    /// The machine service; `None` leaves its host name to the usual egress path.
    #[serde(deserialize_with = "machine_service")]
    pub service: Option<ServedHost>,
    /// What a sync to a machine leaves alone, beside the repo's own `machine.sync`.
    pub sync: SyncConfig,
}

/// What a host the Proxy serves is for: it names the MCP server and what the Proxy stamps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Served {
    /// The machine runner. Requests carry the Box and the machine it holds.
    Machine,
    /// The gate's Box-facing listener. Requests carry the Box.
    Moor,
}

impl Served {
    /// The name a Box's agent knows the MCP server by.
    #[must_use]
    pub const fn mcp_name(self) -> &'static str {
        match self {
            Self::Machine => "machine",
            Self::Moor => "moor",
        }
    }

    /// The config key that sets it.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Machine => "machines.service",
            Self::Moor => "moor_service",
        }
    }

    /// The host a Box requests when the entry names none.
    #[must_use]
    pub const fn default_host(self) -> &'static str {
        match self {
            Self::Machine => MACHINE_SERVICE_HOST,
            Self::Moor => MOOR_SERVICE_HOST,
        }
    }
}

/// The host name a Box reaches the machine service at when `machines.service` names none.
pub const MACHINE_SERVICE_HOST: &str = "machine.moor.internal";

/// The host name a Box reaches moor's own MCP server at when `moor_service` names none.
pub const MOOR_SERVICE_HOST: &str = "moor.moor.internal";

/// A host name the Proxy serves itself: a Box's plain `http://` requests for it go to
/// `upstream`, stamped with who sent them, and every Box's agent gets it as an MCP server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServedHost {
    /// What it is for.
    pub served: Served,
    /// Name a Box requests over `http://`, compared without case.
    pub host: String,
    /// Where the Proxy sends those requests.
    pub upstream: Upstream,
}

impl ServedHost {
    /// True when `host` (no port) names this service.
    #[must_use]
    pub fn serves(&self, host: &str) -> bool {
        host.eq_ignore_ascii_case(&self.host)
    }

    /// The service's MCP server, reached over plain `http://` so the Proxy can route it.
    #[must_use]
    pub fn mcp_server(&self) -> McpServer {
        McpServer {
            kind: Some("http".to_owned()),
            url: Some(format!("http://{}/mcp", self.host)),
            ..McpServer::default()
        }
    }

    /// `host` is a DNS name.
    pub(super) fn validate(&self) -> Result<(), ConfigError> {
        dns_name(&self.host).then_some(()).ok_or_else(|| {
            ConfigError::new(
                format!("{}.host", self.served.key()),
                format!("must be a DNS name such as {}", self.served.default_host()),
            )
        })
    }
}

/// `host:port` the Proxy dials for a served host. `host` is a DNS name, looked up on each
/// connection, or an IP address; either way every address it reaches is loopback or private.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Upstream(String);

impl Upstream {
    /// `host:port` whose host is a DNS name or an address [`reachable`] allows.
    ///
    /// # Errors
    /// Why `text` is no such upstream.
    pub fn parse(text: &str) -> Result<Self, String> {
        let (host, port) = text
            .rsplit_once(':')
            .ok_or_else(|| format!("{text:?} is not host:port"))?;
        if port.parse::<u16>().map_or(true, |p| p == 0) {
            return Err(format!("{text:?} is not host:port"));
        }
        let bare = host.strip_prefix('[').and_then(|h| h.strip_suffix(']'));
        match bare.unwrap_or(host).parse::<IpAddr>() {
            Ok(ip) if reachable(ip) => Ok(Self(text.to_owned())),
            Ok(_) => Err(format!(
                "{text:?} is not a loopback or private address; a served host's upstream \
                 never leaves this host or its cluster"
            )),
            Err(_) if bare.is_none() && dns_name(host) => Ok(Self(text.to_owned())),
            Err(_) => Err(format!("{text:?} is not host:port")),
        }
    }

    /// `host:port` as configured.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Upstream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Upstream {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let text = String::deserialize(de)?;
        Self::parse(&text).map_err(|why| {
            serde::de::Error::custom(format!(
                "{why}; use an address and port such as 127.0.0.1:7700 or \
                 moor-gate.moor.svc:8444"
            ))
        })
    }
}

/// True for an address a served host may reach, on this host or its cluster's network.
///
/// That is loopback, RFC 1918, carrier-grade NAT (`100.64.0.0/10`), where cluster pods and
/// Services live, or IPv6 unique local (`fc00::/7`). Link-local, unspecified and every public
/// address are refused.
#[must_use]
pub fn reachable(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            v4.is_loopback() || v4.is_private() || (a == 100 && (64..128).contains(&b))
        }
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or_else(
            || v6.is_loopback() || (v6.segments()[0] & 0xfe00) == 0xfc00,
            |v4| reachable(IpAddr::V4(v4)),
        ),
    }
}

/// The fields of a served host's entry as written.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    /// Name a Box requests; the kind's default when absent.
    host: Option<String>,
    /// Where the Proxy sends those requests.
    upstream: Upstream,
}

impl Entry {
    /// The served host this entry sets for `served`.
    fn of(self, served: Served) -> ServedHost {
        ServedHost {
            served,
            host: self
                .host
                .unwrap_or_else(|| served.default_host().to_owned()),
            upstream: self.upstream,
        }
    }
}

/// Reads `machines.service`.
fn machine_service<'de, D: Deserializer<'de>>(de: D) -> Result<Option<ServedHost>, D::Error> {
    Ok(Option::<Entry>::deserialize(de)?.map(|e| e.of(Served::Machine)))
}

/// Reads `moor_service`.
pub(super) fn moor_service<'de, D: Deserializer<'de>>(
    de: D,
) -> Result<Option<ServedHost>, D::Error> {
    Ok(Option::<Entry>::deserialize(de)?.map(|e| e.of(Served::Moor)))
}

/// True when `host` is a DNS name.
fn dns_name(host: &str) -> bool {
    let label_ok = |l: &str| {
        !l.is_empty()
            && l.len() <= 63
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    };
    host.len() <= 253 && host.split('.').all(label_ok)
}
