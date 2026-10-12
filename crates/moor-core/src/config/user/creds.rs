//! User `credentials:` and `mcp_servers:` sections.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Deserialize;

use crate::config::{check, ConfigError};

/// Where a credential value comes from: exactly one of `cmd`, `file`, `env`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct CredentialSource {
    /// Argv of a command printing the value.
    pub cmd: Option<Vec<String>>,
    /// File holding the value.
    pub file: Option<PathBuf>,
    /// Host environment variable holding the value.
    pub env: Option<String>,
    /// Host variables a `cmd` may read, on top of the process allow-list.
    pub pass_env: Vec<String>,
}

impl CredentialSource {
    /// Exactly one non-blank source is given.
    fn validate(&self) -> Result<(), &'static str> {
        let given: Vec<bool> = [
            self.cmd
                .as_ref()
                .map(|v| v.first().is_none_or(|p| p.trim().is_empty())),
            self.file
                .as_ref()
                .map(|p| p.to_string_lossy().trim().is_empty()),
            self.env.as_deref().map(|s| s.trim().is_empty()),
        ]
        .into_iter()
        .flatten()
        .collect();
        match given.as_slice() {
            [] => Err("needs exactly one of cmd, file, env"),
            [false] => Ok(()),
            [true] => Err("source must not be blank"),
            _ => Err("needs exactly one of cmd, file, env, got several"),
        }
    }
}

impl CredentialSource {
    /// `pass_env` rides on a `cmd`, holds valid names, and lists none twice.
    fn validate_pass_env(&self, at: &str) -> Result<(), ConfigError> {
        if self.pass_env.is_empty() {
            return Ok(());
        }
        let at = format!("{at}.pass_env");
        if self.cmd.is_none() {
            return Err(ConfigError::new(at, "is only allowed with cmd"));
        }
        let mut seen = std::collections::BTreeSet::new();
        for (i, name) in self.pass_env.iter().enumerate() {
            let here = format!("{at}[{i}]");
            check::env_name(name).map_err(|why| ConfigError::new(here.clone(), why))?;
            if !seen.insert(name) {
                return Err(ConfigError::new(here, "listed twice"));
            }
        }
        Ok(())
    }
}

/// An MCP server: exactly one of `command` (stdio) or `url`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct McpServer {
    /// Transport name (`http`, `sse`, `stdio`); not validated.
    #[serde(rename = "type")]
    pub kind: Option<String>,
    /// Program to run.
    pub command: Option<String>,
    /// Program arguments; may hold `${CREDENTIAL}` refs.
    pub args: Vec<String>,
    /// Program environment; values may hold `${CREDENTIAL}` refs.
    pub env: BTreeMap<String, String>,
    /// Server URL; may hold `${CREDENTIAL}` refs.
    pub url: Option<String>,
    /// Request headers; values may hold `${CREDENTIAL}` refs.
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

/// Validates the `credentials` map.
pub(super) fn validate_credentials(
    creds: &BTreeMap<String, CredentialSource>,
) -> Result<(), ConfigError> {
    for (name, src) in creds {
        let at = format!("credentials.{name}");
        check::not_gh_token(name).map_err(|why| ConfigError::new(at.clone(), why))?;
        check::credential_name(name).map_err(|why| ConfigError::new(at.clone(), why))?;
        src.validate()
            .map_err(|why| ConfigError::new(at.clone(), why))?;
        src.validate_pass_env(&at)?;
    }
    Ok(())
}

/// Validates the `mcp_servers` map against `creds`.
pub(super) fn validate_mcp(
    servers: &BTreeMap<String, McpServer>,
    creds: &BTreeMap<String, CredentialSource>,
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
                if !creds.contains_key(r) {
                    return Err(ConfigError::new(
                        format!("{at}.{key}"),
                        format!("${{{r}}} is not an entry of credentials"),
                    ));
                }
            }
        }
    }
    Ok(())
}
