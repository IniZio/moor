//! `proxy.direct`: hosts the Box reaches without the Proxy.

use crate::config::ConfigError;
use crate::name::DirectHost;

/// Public package registries the Box reaches without the Proxy unless the user turns them off.
///
/// None of them commonly carries a credential, so ghcr.io, github.com and gitlab.com stay out.
pub const BUILTIN_DIRECT: [&str; 18] = [
    "registry.npmjs.org",
    "registry.yarnpkg.com",
    "pypi.org",
    "files.pythonhosted.org",
    "crates.io",
    "static.crates.io",
    "index.crates.io",
    "repo.maven.apache.org",
    "repo1.maven.org",
    ".gradle.org",
    "proxy.golang.org",
    "sum.golang.org",
    "rubygems.org",
    "deb.debian.org",
    "security.debian.org",
    "archive.ubuntu.com",
    "security.ubuntu.com",
    "dl.google.com",
];

/// The built-in direct hosts, without any that covers one of `rule_hosts`, so those stay proxied.
pub(super) fn builtin_direct(rule_hosts: &[&str]) -> Vec<DirectHost> {
    BUILTIN_DIRECT
        .iter()
        .filter_map(|h| DirectHost::try_from((*h).to_owned()).ok())
        .filter(|d| !rule_hosts.iter().any(|h| d.covers(h)))
        .collect()
}

/// Refuses a `direct` entry that covers any of `rule_hosts`, the hosts a credential rule serves.
///
/// # Errors
/// Returns [`ConfigError`] at `{at}[i]` naming the entry and the rule host.
pub fn check_direct(
    direct: &[DirectHost],
    at: &str,
    rule_hosts: &[&str],
) -> Result<(), ConfigError> {
    for (i, entry) in direct.iter().enumerate() {
        if let Some(host) = rule_hosts.iter().find(|h| entry.covers(h)) {
            return Err(ConfigError::new(
                format!("{at}[{i}]"),
                format!(
                    "{} covers {host}, which a credential rule serves; it must go through the Proxy",
                    entry.no_proxy()
                ),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
