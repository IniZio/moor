//! Egress allow-list and the `moor env` egress item, computed once from settings.

use super::ConfigError;
use crate::config::check;
use crate::config::proxy::default_rules;

/// How far a provider can restrict Box egress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EgressSupport {
    /// Always enforced.
    Enforced,
    /// Enforced when requested.
    OptIn,
    /// Not available.
    None,
}

/// Hosts every Box may reach.
pub const BASE_ALLOW: &[&str] = &[
    "*.githubusercontent.com",
    "registry.npmjs.org",
    "mise.jdx.dev",
    "mise-versions.jdx.dev",
    "mise.run",
    "nodejs.org",
    "registry-1.docker.io",
    "auth.docker.io",
    "production.cloudflare.docker.com",
    "download.docker.com",
    "deb.debian.org",
    "security.debian.org",
    "archive.ubuntu.com",
    "security.ubuntu.com",
];

/// Note text for a provider that fixes the allow-list at creation, without the opt-in.
const UNENFORCED: &str = "setup and moor run have open egress; the agent reaches only the Proxy";

/// Note text for a provider with no egress control when `box.enforce_egress` is set.
const CONFLICT: &str =
    "box.enforce_egress is set but this provider cannot enforce egress; start will refuse";

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

/// Base list, then the agent's hosts, then user `egress.allow`, then repo entries; trimmed,
/// lower-cased, de-duplicated by exact string.
///
/// `*.x` never absorbs `x`. The hosts of the default tool rules are served by
/// the moor proxy, so they are not in the base list and an entry for one is an error.
///
/// # Errors
/// An entry that is not a domain or `*.domain`, or that a default tool rule serves.
/// The error path is `egress.allow[i]` for user entries and `egress[i]` for repo entries.
pub fn effective_allow_list(
    agent: &[&str],
    user: &[String],
    repo: &[String],
) -> Result<Vec<String>, ConfigError> {
    let mut out: Vec<String> = BASE_ALLOW
        .iter()
        .chain(agent)
        .map(|s| (*s).to_string())
        .collect();
    let served: Vec<String> = default_rules()
        .into_values()
        .flat_map(|rule| rule.hosts)
        .map(|h| h.to_ascii_lowercase())
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
        if served.contains(&entry) {
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

/// Whether a provider with `support` applies an allow-list; opt-in support needs `enforce_egress`.
#[must_use]
pub const fn provider_applies_policy(support: EgressSupport, enforce_egress: bool) -> bool {
    match support {
        EgressSupport::Enforced => true,
        EgressSupport::OptIn => enforce_egress,
        EgressSupport::None => false,
    }
}

/// Note when egress is not enforced as configured: opt-in support without the opt-in, or the
/// opt-in on a provider with no support.
#[must_use]
pub const fn unenforced_note(support: EgressSupport, enforce_egress: bool) -> Option<&'static str> {
    match support {
        EgressSupport::OptIn if !enforce_egress => Some(UNENFORCED),
        EgressSupport::None if enforce_egress => Some(CONFLICT),
        _ => None,
    }
}

/// The single egress row for `moor env`; `hosts` is [`effective_allow_list`].
///
/// # Errors
/// Same as [`effective_allow_list`]; returned only when the provider applies a policy.
pub fn egress_item(
    agent: &[&str],
    user: &[String],
    repo: &[String],
    provider_applies_policy: bool,
) -> Result<EgressItem, ConfigError> {
    let list = effective_allow_list(agent, user, repo);
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
