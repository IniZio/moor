//! MCP servers a moor user adds for their own Boxes, and how they merge with the shared
//! `mcp_servers:` of the operator's config.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::creds::{https_host, McpServer};
use crate::config::check;
use crate::user::SecretName;

/// The header a user's server gets its key in when none is named.
pub const DEFAULT_HEADER: &str = "Authorization";

/// One of a user's own MCP servers. Its key lives in the user's vault as [`UserMcp::secret`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserMcp {
    /// The server's name, unique per user.
    pub name: String,
    /// The `https://` URL the agent connects to.
    pub url: String,
    /// The header the Proxy sets the key in.
    pub header: String,
}

impl UserMcp {
    /// Checks a server a user adds.
    ///
    /// # Errors
    /// Why the name, URL or header is not accepted.
    pub fn parse(name: &str, url: &str, header: Option<&str>) -> Result<Self, String> {
        check::credential_name(name)?;
        SecretName::parse(name).map_err(|_| {
            format!("server name `{name}` must match [a-z][a-z0-9_-]* in at most 64 bytes")
        })?;
        match (https_host(url), local_http_host(url)) {
            (Some(host), _) => check::host_entry(host)?,
            (None, Some(_)) => {}
            (None, None) => return Err(format!("server URL `{url}` must be https:// with a host")),
        }
        let header = header.unwrap_or(DEFAULT_HEADER);
        if header.is_empty() || !header.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            return Err(format!("header `{header}` must match [A-Za-z0-9-]+"));
        }
        Ok(Self {
            name: name.to_owned(),
            url: url.to_owned(),
            header: header.to_owned(),
        })
    }

    /// The vault secret that holds this server's key.
    #[must_use]
    pub fn secret(&self) -> SecretName {
        SecretName::parse(&format!("mcp_{}", self.name))
            .unwrap_or_else(|_| unreachable!("a parsed server name makes a valid secret name"))
    }

    /// The server in `mcp_servers` form. `Authorization` carries `Bearer <key>`; any other header
    /// carries the key alone.
    #[must_use]
    pub fn server(&self) -> McpServer {
        let secret = self.secret();
        let value = if self.header.eq_ignore_ascii_case(DEFAULT_HEADER) {
            format!("Bearer ${{{}}}", secret.as_str())
        } else {
            format!("${{{}}}", secret.as_str())
        };
        McpServer {
            kind: Some("http".to_owned()),
            url: Some(self.url.clone()),
            headers: BTreeMap::from([(self.header.clone(), value)]),
            ..McpServer::default()
        }
    }
}

/// `http://localhost` and `http://127.0.0.1` hosts, for servers run beside the controller.
fn local_http_host(url: &str) -> Option<&str> {
    let rest = url.strip_prefix("http://")?;
    let host = rest.split(['/', '?', '#']).next()?.split(':').next()?;
    matches!(host, "localhost" | "127.0.0.1").then_some(host)
}

/// The MCP servers one Box gets, still naming secrets, and the start notes about them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BoxMcp {
    /// Servers by name; a header may still name a secret the Proxy injects.
    pub servers: BTreeMap<String, McpServer>,
    /// What the requester should know: replaced shared servers, servers left out.
    pub notes: Vec<String>,
}

/// The shared servers merged with the user's `own`: the user's server wins a name clash. A
/// server naming a secret the user lacks (`have`) is left out, never refusing the start.
#[must_use]
pub fn box_mcp(
    shared: &BTreeMap<String, McpServer>,
    own: &[UserMcp],
    have: &BTreeSet<SecretName>,
) -> BoxMcp {
    let mut servers = shared.clone();
    let mut replaced = Vec::new();
    for o in own {
        if servers.insert(o.name.clone(), o.server()).is_some() {
            replaced.push(o.name.as_str());
        }
    }
    let mut left_out = Vec::new();
    servers.retain(|name, s| {
        let held = s
            .refs()
            .iter()
            .all(|r| SecretName::parse(r).is_ok_and(|n| have.contains(&n)));
        if !held {
            left_out.push(name.clone());
        }
        held
    });
    let mut notes = Vec::new();
    if !replaced.is_empty() {
        notes.push(format!(
            "your MCP server {} replaces the shared one of that name",
            replaced.join(", ")
        ));
    }
    if !left_out.is_empty() {
        notes.push(format!(
            "MCP server {} left out: no key stored for it (add one with /moor mcp add)",
            left_out.join(", ")
        ));
    }
    BoxMcp { servers, notes }
}

#[cfg(test)]
mod tests;
