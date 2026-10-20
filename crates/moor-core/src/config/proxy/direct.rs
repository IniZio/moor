//! `proxy.direct`: hosts the Box reaches without the Proxy.

use crate::config::ConfigError;
use crate::name::DirectHost;

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
