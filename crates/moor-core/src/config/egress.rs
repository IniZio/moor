//! Egress allow-list and the `moor env` egress item, computed once from settings.

use super::ConfigError;
use crate::config::check;

/// Provider that takes the allow-list as a flag; always enforces.
pub const FLAG_PROVIDER: &str = "sprites";
/// Provider that takes the allow-list as an env var; enforces only on opt-in.
pub const ENV_PROVIDER: &str = "namespace-devbox";

/// Hosts every Box may reach.
pub const BASE_ALLOW: &[&str] = &[
    "github.com",
    "api.github.com",
    "*.githubusercontent.com",
    "registry.npmjs.org",
    "mise.jdx.dev",
    "mise-versions.jdx.dev",
    "mise.run",
    "nodejs.org",
    "api.anthropic.com",
    "registry-1.docker.io",
    "auth.docker.io",
    "production.cloudflare.docker.com",
    "download.docker.com",
    "deb.debian.org",
    "security.debian.org",
    "archive.ubuntu.com",
    "security.ubuntu.com",
];

/// Hosts a Box agent reaches only through the moor proxy.
const PROXY_SERVED: &[&str] = &["api.github.com", "api.anthropic.com"];

/// Note text for namespace-devbox without the opt-in.
const UNENFORCED: &str = "egress not enforced on namespace-devbox (allow-list is fixed at Box creation; set box.enforce_egress)";

/// One `moor env` egress row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EgressItem {
    /// `allow` or `policy`.
    pub name: &'static str,
    /// Human text for the row.
    pub detail: String,
    /// The effective allow list; empty when it is invalid and the provider ignores it.
    pub hosts: Vec<String>,
}

/// Base list, then user `egress.allow`, then repo entries; trimmed, lower-cased,
/// de-duplicated by exact string.
///
/// `*.x` never absorbs `x`. The proxy-served base hosts are dropped and an
/// entry for one is an error.
///
/// # Errors
/// An entry that is not a domain or `*.domain`, or that the proxy serves.
/// The error path is `egress.allow[i]` for user entries and `egress[i]` for repo entries.
pub fn effective_allow_list(user: &[String], repo: &[String]) -> Result<Vec<String>, ConfigError> {
    let mut out: Vec<String> = BASE_ALLOW
        .iter()
        .filter(|h| !PROXY_SERVED.contains(h))
        .map(|s| (*s).to_string())
        .collect();
    let entries = user
        .iter()
        .enumerate()
        .map(|(i, r)| (format!("egress.allow[{i}]"), r))
        .chain(
            repo.iter()
                .enumerate()
                .map(|(i, r)| (format!("egress[{i}]"), r)),
        );
    for (path, raw) in entries {
        check::egress_entry(raw).map_err(|msg| ConfigError::new(path.clone(), msg))?;
        let entry = raw.trim().to_ascii_lowercase();
        if PROXY_SERVED.contains(&entry.as_str()) {
            return Err(ConfigError::new(
                path,
                format!(
                    "entry {raw:?} is not allowed with the proxy active: the moor proxy serves it"
                ),
            ));
        }
        if !out.contains(&entry) {
            out.push(entry);
        }
    }
    Ok(out)
}

/// Whether `provider` can apply an allow-list; namespace needs `enforce_egress`.
#[must_use]
pub fn provider_applies_policy(provider: &str, enforce_egress: bool) -> bool {
    provider == FLAG_PROVIDER || (provider == ENV_PROVIDER && enforce_egress)
}

/// Warning when namespace-devbox runs without the egress opt-in.
#[must_use]
pub fn unenforced_note(provider: &str, enforce_egress: bool) -> Option<&'static str> {
    (provider == ENV_PROVIDER && !enforce_egress).then_some(UNENFORCED)
}

/// The single egress row for `moor env`; `hosts` is [`effective_allow_list`].
///
/// # Errors
/// Same as [`effective_allow_list`]; returned only when the provider applies a policy.
pub fn egress_item(
    user: &[String],
    repo: &[String],
    provider_applies_policy: bool,
) -> Result<EgressItem, ConfigError> {
    let list = effective_allow_list(user, repo);
    if provider_applies_policy {
        let hosts = list?;
        let detail = hosts.join(", ");
        return Ok(EgressItem {
            name: "allow",
            detail,
            hosts,
        });
    }
    let n = user.len() + repo.len();
    let detail = if n == 0 {
        "no egress policy".to_string()
    } else {
        let noun = if n == 1 { "entry" } else { "entries" };
        format!("no egress policy (Box provider cannot apply {n} egress {noun})")
    };
    Ok(EgressItem {
        name: "policy",
        detail,
        hosts: list.unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests;
