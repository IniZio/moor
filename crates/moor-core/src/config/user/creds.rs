//! User `mcp_servers:` section.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::config::{check, ConfigError};
use crate::user::SecretName;

/// An MCP server: exactly one of `command` (stdio) or `url`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct McpServer {
    /// Transport name (`http`, `sse`, `stdio`); not validated.
    #[serde(rename = "type")]
    pub kind: Option<String>,
    /// Program to run.
    pub command: Option<String>,
    /// Program arguments; may hold `${SECRET}` refs.
    pub args: Vec<String>,
    /// Program environment; values may hold `${SECRET}` refs.
    pub env: BTreeMap<String, String>,
    /// Server URL; may hold `${SECRET}` refs.
    pub url: Option<String>,
    /// Request headers; values may hold `${SECRET}` refs.
    pub headers: BTreeMap<String, String>,
}

/// `${NAME}` references in `text`, in order.
fn credential_refs(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some((_, after)) = rest.split_once("${") {
        let Some((name, tail)) = after.split_once('}') else {
            break;
        };
        out.push(name);
        rest = tail;
    }
    out
}

impl McpServer {
    /// Every checked text with its key path under the server.
    fn texts(&self) -> Vec<(String, &str)> {
        let mut out = Vec::new();
        for (i, a) in self.args.iter().enumerate() {
            out.push((format!("args[{i}]"), a.as_str()));
        }
        for (k, v) in &self.env {
            out.push((format!("env.{k}"), v.as_str()));
        }
        if let Some(u) = &self.url {
            out.push(("url".to_owned(), u.as_str()));
        }
        for (k, v) in &self.headers {
            out.push((format!("headers.{k}"), v.as_str()));
        }
        out
    }

    /// Distinct credential names referenced, sorted.
    #[must_use]
    pub fn refs(&self) -> Vec<&str> {
        let mut out: Vec<&str> = self
            .texts()
            .into_iter()
            .flat_map(|(_, t)| credential_refs(t))
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// Validates the `mcp_servers` map; each `${NAME}` names a secret of the Box's user that no
/// Proxy rule in `injected` injects.
pub(super) fn validate_mcp(
    servers: &BTreeMap<String, McpServer>,
    injected: &BTreeMap<String, String>,
) -> Result<(), ConfigError> {
    for (name, s) in servers {
        let at = format!("mcp_servers.{name}");
        check::credential_name(name).map_err(|why| ConfigError::new(at.clone(), why))?;
        match (s.command.is_some(), s.url.is_some()) {
            (true, false) | (false, true) => {}
            (false, false) => {
                return Err(ConfigError::new(at, "needs exactly one of command, url"));
            }
            (true, true) => {
                return Err(ConfigError::new(
                    at,
                    "needs exactly one of command, url, got both",
                ));
            }
        }
        for (key, text) in s.texts() {
            for r in credential_refs(text) {
                SecretName::parse(r)
                    .map(|_| ())
                    .and_then(|()| check::not_injected(r, injected))
                    .map_err(|why| {
                        ConfigError::new(format!("{at}.{key}"), format!("${{{r}}}: {why}"))
                    })?;
            }
        }
    }
    Ok(())
}
