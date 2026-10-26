//! The `machines:` section of the user config: the host-local service a Box reaches, through
//! its Proxy, for the machine it holds.

use std::net::SocketAddr;

use serde::{Deserialize, Deserializer};

use crate::config::ConfigError;

/// `machines:` section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct MachinesConfig {
    /// The machine service; `None` leaves its host name to the usual egress path.
    pub service: Option<MachineService>,
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

    /// `host` is a DNS name.
    pub(super) fn validate(&self) -> Result<(), ConfigError> {
        let label_ok = |l: &str| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        };
        if self.host.len() > 253 || !self.host.split('.').all(label_ok) {
            return Err(ConfigError::new(
                "machines.service.host",
                "must be a DNS name such as machine.moor.internal",
            ));
        }
        Ok(())
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
