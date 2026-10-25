//! User `mcp_servers:` section.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::config::proxy::{Inject, ToolRule};
use crate::config::{check, ConfigError};
use crate::user::SecretName;

/// What the Box sees where a header names a secret; the Proxy sets the real header on the wire.
const PROXY_PLACEHOLDER: &str = "moor-proxy";

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
    /// Request headers; one value may hold one `${SECRET}` ref, which the Proxy injects.
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

    /// The header that names a secret: its name, its value, and the secret.
    fn secret_header(&self) -> Option<(&str, &str, &str)> {
        self.headers.iter().find_map(|(k, v)| {
            credential_refs(v)
                .first()
                .map(|r| (k.as_str(), v.as_str(), *r))
        })
    }

    /// This server as the Box sees it: a header that names a secret holds the placeholder.
    #[must_use]
    pub fn in_box(&self) -> Self {
        let mut out = self.clone();
        if let Some((key, value, secret)) = self.secret_header() {
            let placeholder = value.replace(&format!("${{{secret}}}"), PROXY_PLACEHOLDER);
            out.headers.insert(key.to_owned(), placeholder);
        }
        out
    }

    /// The Proxy rule that sets the secret-naming header on requests to the server's host.
    #[must_use]
    #[expect(
        clippy::literal_string_with_formatting_args,
        reason = "`{secret}` is the Proxy's form slot"
    )]
    pub fn proxy_rule(&self) -> Option<ToolRule> {
        let (key, value, secret) = self.secret_header()?;
        let host = https_host(self.url.as_deref()?)?;
        Some(ToolRule {
            hosts: vec![host.to_owned()],
            credential: secret.to_owned(),
            inject: Inject {
                header: Some(key.to_owned()),
                form: Some(value.replace(&format!("${{{secret}}}"), "{secret}")),
                basic_user: None,
            },
            allow: vec!["*".to_owned()],
            ..ToolRule::default()
        })
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

/// The host of an `https://` URL, without its port.
pub(super) fn https_host(url: &str) -> Option<&str> {
    let rest = url.strip_prefix("https://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.split(':').next()?;
    (!host.is_empty() && !authority.contains('@')).then_some(host)
}

/// The Proxy rule name of the server `name`.
#[must_use]
pub(super) fn rule_name(name: &str) -> String {
    format!("mcp-{name}")
}

/// Validates the `mcp_servers` map; each `${NAME}` names a secret of the Box's user that no
/// Proxy rule in `injected` injects.
pub(super) fn validate_mcp(
    servers: &BTreeMap<String, McpServer>,
    injected: &BTreeMap<String, String>,
    rules: &BTreeMap<String, ToolRule>,
) -> Result<(), ConfigError> {
    for (name, s) in servers {
        let at = format!("mcp_servers.{name}");
        check::credential_name(name).map_err(|why| ConfigError::new(at.clone(), why))?;
        if rules.contains_key(&rule_name(name)) {
            return Err(ConfigError::new(
                at,
                format!(
                    "proxy.rules.{} is the rule moor makes for this server",
                    rule_name(name)
                ),
            ));
        }
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
                    .and_then(|()| {
                        if key.starts_with("headers.") {
                            Ok(())
                        } else {
                            Err("only a header may name a secret, so the Proxy injects it and the Box never holds it".to_owned())
                        }
                    })
                    .map_err(|why| {
                        ConfigError::new(format!("{at}.{key}"), format!("${{{r}}}: {why}"))
                    })?;
            }
        }
        check_secret_header(s, &at)?;
    }
    Ok(())
}

/// At most one header names a secret, with one ref, on an `https://` server.
fn check_secret_header(s: &McpServer, at: &str) -> Result<(), ConfigError> {
    let named: Vec<(&String, usize)> = s
        .headers
        .iter()
        .map(|(k, v)| (k, credential_refs(v).len()))
        .filter(|(_, n)| *n > 0)
        .collect();
    match named.as_slice() {
        [] => Ok(()),
        [(_, 1)] => {
            let host = s.url.as_deref().and_then(https_host).ok_or_else(|| {
                ConfigError::new(
                    format!("{at}.url"),
                    "must be an https:// URL with a host when a header names a secret",
                )
            })?;
            check::host_entry(host).map_err(|why| ConfigError::new(format!("{at}.url"), why))?;
            Ok(())
        }
        [(key, _)] => Err(ConfigError::new(
            format!("{at}.headers.{key}"),
            "may name one secret",
        )),
        _ => Err(ConfigError::new(
            format!("{at}.headers"),
            "only one header may name a secret",
        )),
    }
}
