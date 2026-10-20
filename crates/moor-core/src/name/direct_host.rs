//! A host the Box reaches without the Proxy.

use serde::Deserialize;

/// A `proxy.direct` entry: `host.name`, or `.domain` for every name under the domain.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct DirectHost {
    /// Lower-cased domain, without the leading dot.
    domain: String,
    /// Written as `.domain`.
    suffix: bool,
}

impl TryFrom<String> for DirectHost {
    type Error = String;

    /// Accepts a dotted name of letters, digits and `-`, optionally after one leading `.`; no
    /// wildcards, IPs, schemes or ports.
    fn try_from(entry: String) -> Result<Self, String> {
        let lower = entry.trim().to_ascii_lowercase();
        let (domain, suffix) = lower
            .strip_prefix('.')
            .map_or((lower.as_str(), false), |d| (d, true));
        let ok = domain.contains('.')
            && !domain.starts_with('.')
            && domain
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
            && domain.parse::<std::net::IpAddr>().is_err();
        if !ok {
            return Err(format!(
                "direct host must be a host name or .domain: {entry:?}"
            ));
        }
        Ok(Self {
            domain: domain.to_owned(),
            suffix,
        })
    }
}

impl DirectHost {
    /// The `NO_PROXY` form: `.domain` or the host name.
    #[must_use]
    pub fn no_proxy(&self) -> String {
        if self.suffix {
            format!(".{}", self.domain)
        } else {
            self.domain.clone()
        }
    }

    /// The Java `nonProxyHosts` form: `*.domain` or the host name.
    #[must_use]
    pub fn java(&self) -> String {
        if self.suffix {
            format!("*.{}", self.domain)
        } else {
            self.domain.clone()
        }
    }

    /// Whether a client could send a request for `host` around the Proxy because of this entry.
    ///
    /// Either form covers the domain and every name under it: curl and Go read a bare `NO_PROXY`
    /// name that way, and curl reads `.domain` as covering the domain itself.
    #[must_use]
    pub fn covers(&self, host: &str) -> bool {
        let host = host.to_ascii_lowercase();
        host == self.domain
            || host
                .strip_suffix(self.domain.as_str())
                .is_some_and(|rest| rest.ends_with('.'))
    }
}
