//! The `machines:` section of the user config: the host-local service a Box reaches, through
//! its Proxy, for the machine it holds.

use std::net::SocketAddr;

use serde::{Deserialize, Deserializer};

use super::McpServer;
use crate::config::{ConfigError, SyncConfig};

/// `machines:` section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct MachinesConfig {
    /// The machine service; `None` leaves its host name to the usual egress path.
    pub service: Option<MachineService>,
    /// What a sync to a machine leaves alone, beside the repo's own `machine.sync`.
    pub sync: SyncConfig,
}

/// `machines.service:` a host name the Proxy serves itself, from a loopback address.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MachineService {
    /// Name a Box requests over `http://`, compared without case.
    pub host: String,
    /// Where the Proxy sends those requests; always a loopback address.
    #[serde(deserialize_with = "loopback")]
    pub upstream: SocketAddr,
}

impl MachineService {
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
        dns_name(&self.host, "machines.service.host", "machine.moor.internal")
    }
}

/// Refuses `host` unless it is a DNS name; `key` and `example` shape the message.
fn dns_name(host: &str, key: &str, example: &str) -> Result<(), ConfigError> {
    let label_ok = |l: &str| {
        !l.is_empty()
            && l.len() <= 63
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    };
    if host.len() > 253 || !host.split('.').all(label_ok) {
        return Err(ConfigError::new(
            key,
            format!("must be a DNS name such as {example}"),
        ));
    }
    Ok(())
}

/// The host name a Box reaches moor's own MCP server at when `moor_service` names none.
pub const MOOR_SERVICE_HOST: &str = "moor.moor.internal";

/// `moor_service:` the MCP server a Box's agent calls `report_step` on. The Proxy routes the
/// host to the gate's Box listener and stamps `X-Moor-Box`; the Box holds no token.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct MoorService {
    /// Name a Box requests over `http://`.
    pub host: String,
}

impl Default for MoorService {
    fn default() -> Self {
        Self {
            host: MOOR_SERVICE_HOST.to_owned(),
        }
    }
}

impl MoorService {
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
        dns_name(&self.host, "moor_service.host", MOOR_SERVICE_HOST)
    }
}

/// Parses `ip:port` and refuses any address off loopback, so the service never leaves the host.
fn loopback<'de, D: Deserializer<'de>>(de: D) -> Result<SocketAddr, D::Error> {
    let text = String::deserialize(de)?;
    text.parse::<SocketAddr>()
        .ok()
        .filter(|a| a.ip().is_loopback())
        .ok_or_else(|| {
            serde::de::Error::custom(format!(
                "{text:?} is not a loopback address and port such as 127.0.0.1:7700; \
                 the machine service must run on this host"
            ))
        })
}
