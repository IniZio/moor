//! `proxy.rules`: which hosts the moor proxy serves, with what credential and path policy.
//!
//! Pure data and validation; the proxy turns [`ResolvedRule`]s into routes.

mod resolve;
mod template;

use std::collections::BTreeMap;

use serde::Deserialize;

use super::{check, parse_strict, ConfigError, CredentialSource};
pub use resolve::{default_rules, resolve_rules, EndpointRule, ResolvedInject, ResolvedRule};
use template::{check_slots, FORM_SLOTS, REPO_SLOTS};

/// Credential name that means the Task's forge token rather than a `credentials:` entry.
pub const FORGE_CREDENTIAL: &str = "forge";

/// Header carrying the credential when `inject.header` is unset.
const DEFAULT_HEADER: &str = "Authorization";

/// How the credential is added to a request.
///
/// Either `form` (a header value template, with `header` defaulting to
/// `Authorization`) or `basic_user` (HTTP Basic with the credential as password).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Inject {
    /// Header name.
    pub header: Option<String>,
    /// Header value template; `{secret}` is the credential.
    pub form: Option<String>,
    /// Basic-auth user name.
    pub basic_user: Option<String>,
}

impl Inject {
    /// Exactly one of `form` or `basic_user`, with well-formed parts.
    fn validate(&self, at: &str) -> Result<(), ConfigError> {
        let fail = |key: &str, why: String| ConfigError::new(format!("{at}.{key}"), why);
        match (&self.form, &self.basic_user) {
            (Some(_), Some(_)) => Err(fail(
                "basic_user",
                "cannot be combined with form".to_owned(),
            )),
            (None, None) => Err(fail("form", "needs form or basic_user".to_owned())),
            (Some(form), None) => {
                check_slots(form, FORM_SLOTS).map_err(|w| fail("form", w))?;
                if !form.contains("{secret}") {
                    return Err(fail("form", "must contain {secret}".to_owned()));
                }
                let header = self.header.as_deref().unwrap_or(DEFAULT_HEADER);
                let token = !header.is_empty()
                    && header
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-');
                if token {
                    Ok(())
                } else {
                    Err(fail("header", format!("invalid header name {header:?}")))
                }
            }
            (None, Some(user)) => {
                if self.header.is_some() {
                    return Err(fail(
                        "header",
                        "cannot be combined with basic_user".to_owned(),
                    ));
                }
                if user.trim().is_empty() || user.contains(':') {
                    return Err(fail(
                        "basic_user",
                        "must be non-empty and contain no ':'".to_owned(),
                    ));
                }
                Ok(())
            }
        }
    }
}

/// One `proxy.rules.<name>` entry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ToolRule {
    /// Exact domains the rule serves.
    pub hosts: Vec<String>,
    /// `credentials:` entry name, or `forge`.
    pub credential: String,
    /// How the credential is added.
    pub inject: Inject,
    /// `*` for any request, or `METHODS /path` patterns; empty means no path list.
    ///
    /// `METHODS` is `*` or `GET|POST`-style; in the path `*` matches one
    /// segment and `**` any number. `{repo}` and `{repo:urlenc}` are filled
    /// from the Task repo.
    pub allow: Vec<String>,
    /// Box environment variables; values may use the repo placeholders.
    pub box_env: BTreeMap<String, String>,
}

impl ToolRule {
    /// Whether any allow entry or Box variable needs the Task repo.
    pub(super) fn needs_repo(&self) -> bool {
        self.allow.iter().any(|a| template::uses_repo(a))
            || self.box_env.values().any(|v| template::uses_repo(v))
    }

    /// The credential is `forge` or a `credentials:` entry.
    pub(super) fn check_credential(
        &self,
        at: &str,
        credentials: &BTreeMap<String, CredentialSource>,
    ) -> Result<(), ConfigError> {
        if self.credential == FORGE_CREDENTIAL || credentials.contains_key(&self.credential) {
            return Ok(());
        }
        Err(ConfigError::new(
            format!("{at}.credential"),
            format!(
                "unknown credential {:?}: not under credentials: and not {FORGE_CREDENTIAL:?}",
                self.credential
            ),
        ))
    }

    /// Checks everything the types cannot except cross-references.
    fn validate(&self, at: &str) -> Result<(), ConfigError> {
        if self.hosts.is_empty() {
            return Err(ConfigError::new(format!("{at}.hosts"), "must not be empty"));
        }
        for (i, h) in self.hosts.iter().enumerate() {
            let exact = !h.contains('*');
            check::egress_entry(h)
                .and_then(|()| {
                    if exact {
                        Ok(())
                    } else {
                        Err("wildcards are not allowed".to_owned())
                    }
                })
                .map_err(|w| ConfigError::new(format!("{at}.hosts[{i}]"), w))?;
        }
        check::credential_name(&self.credential)
            .map_err(|w| ConfigError::new(format!("{at}.credential"), w))?;
        self.inject.validate(&format!("{at}.inject"))?;
        resolve::validate_allow(&self.allow, at)?;
        for (name, value) in &self.box_env {
            let here = format!("{at}.box_env.{name}");
            check::env_name(name)
                .and_then(|()| check::not_reserved_env(name))
                .map_err(|w| ConfigError::new(here.clone(), w))?;
            check_slots(value, REPO_SLOTS).map_err(|w| ConfigError::new(here, w))?;
        }
        Ok(())
    }
}

/// The `proxy:` section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ProxyConfig {
    /// Rules by name; a rule named like a default replaces it.
    pub rules: BTreeMap<String, ToolRule>,
}

impl ProxyConfig {
    /// Checks each rule, its credential against `credentials`, and its hosts
    /// against the `egress.allow` list.
    pub(super) fn validate(
        &self,
        credentials: &BTreeMap<String, CredentialSource>,
        egress_allow: &[String],
    ) -> Result<(), ConfigError> {
        for (name, rule) in &self.rules {
            let at = format!("proxy.rules.{name}");
            check::credential_name(name).map_err(|w| ConfigError::new(at.clone(), w))?;
            rule.validate(&at)?;
            rule.check_credential(&at, credentials)?;
        }
        let mut served = default_rules();
        served.extend(self.rules.clone());
        for (name, rule) in &served {
            for (j, host) in rule.hosts.iter().enumerate() {
                if let Some(i) = covering_entry(host, egress_allow) {
                    return Err(ConfigError::new(
                        format!("proxy.rules.{name}.hosts[{j}]"),
                        format!(
                            "host {host:?} is also in egress.allow[{i}]; the proxy serves it, remove one"
                        ),
                    ));
                }
            }
        }
        Ok(())
    }
}

/// Index of the first allow entry equal to, or a wildcard covering, `host`.
fn covering_entry(host: &str, allow: &[String]) -> Option<usize> {
    let host = host.trim().to_ascii_lowercase();
    allow.iter().position(|e| {
        let e = e.trim().to_ascii_lowercase();
        e == host
            || e.strip_prefix('*')
                .is_some_and(|suffix| host.ends_with(suffix))
    })
}

/// Parses and validates a standalone rules document (name to rule).
///
/// # Errors
/// Returns [`ConfigError`] for bad YAML, unknown keys, or invalid rules.
pub(super) fn parse_rules(
    file: &str,
    text: &str,
) -> Result<BTreeMap<String, ToolRule>, ConfigError> {
    let rules: BTreeMap<String, ToolRule> = parse_strict(file, text)?;
    for (name, rule) in &rules {
        rule.validate(&format!("proxy.rules.{name}"))
            .map_err(|e| e.in_file(file))?;
    }
    Ok(rules)
}

#[cfg(test)]
mod tests;
