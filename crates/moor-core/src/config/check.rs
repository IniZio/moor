//! Shared pure validators for user and repo config schemas.
//!
//! Each validator returns the one canonical message on failure; callers wrap it
//! in a `ConfigError` and do not reword it.

use std::collections::BTreeMap;

use crate::user::SecretName;

/// `[A-Za-z0-9_-]+`.
///
/// # Errors
/// Returns the canonical message when `s` is empty or has other characters.
pub fn credential_name(s: &str) -> Result<(), String> {
    let ok = !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if ok {
        Ok(())
    } else {
        Err(format!("name {s:?} must match [A-Za-z0-9_-]+"))
    }
}

/// `[A-Za-z_][A-Za-z0-9_]*`.
///
/// # Errors
/// Returns the canonical message when `s` is not an environment variable name.
pub fn env_name(s: &str) -> Result<(), String> {
    let mut chars = s.chars();
    let ok = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
    if ok {
        Ok(())
    } else {
        Err(format!("name {s:?} must match [A-Za-z_][A-Za-z0-9_]*"))
    }
}

/// Variable names a box credential may not target.
///
/// # Errors
/// Returns the canonical message for `PATH`.
pub fn not_reserved_env(s: &str) -> Result<(), String> {
    if s == "PATH" {
        Err(format!("name {s:?} is reserved"))
    } else {
        Ok(())
    }
}

/// True when `s` looks like it names a credential, so it never reaches the agent.
#[must_use]
pub fn credential_like_env(s: &str) -> bool {
    const MARKS: [&str; 6] = ["TOKEN", "SECRET", "KEY", "PASSWORD", "PASSWD", "CREDENTIAL"];
    let upper = s.to_ascii_uppercase();
    MARKS.iter().any(|m| upper.contains(m))
}

/// True when `s` is set by moor for the agent, or can load code into the agent's shell or
/// runtime, so a repo `env` may not set it.
///
/// That includes `GIT_CONFIG_*` and `XDG_CONFIG_HOME`, which can point git at a config that
/// sets `core.fsmonitor`.
#[must_use]
pub fn agent_env_reserved(s: &str) -> bool {
    const FIXED: [&str; 34] = [
        "PATH",
        "HOME",
        "USER",
        "SHELL",
        "TERM",
        "LANG",
        "LC_ALL",
        "TMPDIR",
        "COLORTERM",
        "HTTPS_PROXY",
        "HTTP_PROXY",
        "https_proxy",
        "http_proxy",
        "NO_PROXY",
        "no_proxy",
        "NODE_USE_ENV_PROXY",
        "SSL_CERT_FILE",
        "CURL_CA_BUNDLE",
        "REQUESTS_CA_BUNDLE",
        "GIT_SSL_CAINFO",
        "NODE_EXTRA_CA_CERTS",
        "GCONV_PATH",
        "BASH_ENV",
        "ENV",
        "NODE_OPTIONS",
        "IFS",
        "PS4",
        "SHELLOPTS",
        "BASHOPTS",
        "CDPATH",
        "NODE_PATH",
        "LOCPATH",
        "GLIBC_TUNABLES",
        "IS_SANDBOX",
    ];
    FIXED.contains(&s)
        || s.starts_with("LD_")
        || s == "XDG_CONFIG_HOME"
        || s.starts_with("GIT_CONFIG_")
        || s.starts_with("CLAUDE_")
}

/// Refuses secret `name` when a Proxy rule injects it; `injected` maps each injected secret to
/// the rule that injects it.
///
/// # Errors
/// Returns the canonical message naming the rule.
pub fn not_injected(name: &str, injected: &BTreeMap<String, String>) -> Result<(), String> {
    injected.get(name).map_or(Ok(()), |rule| {
        Err(format!(
            "{name} is the secret the Proxy's {rule} rule injects, so a Box may not name it"
        ))
    })
}

/// Validates a box credential name list in order.
///
/// Each name is the name of a secret of the Box's user that no Proxy rule in `injected`
/// injects, and may appear only once.
///
/// # Errors
/// Returns the index of the first failing name and its canonical message.
pub fn box_credentials(
    names: &[String],
    injected: &BTreeMap<String, String>,
) -> Result<(), (usize, String)> {
    for (i, name) in names.iter().enumerate() {
        SecretName::parse(name)
            .map(|_| ())
            .and_then(|()| not_injected(name, injected))
            .map_err(|msg| (i, msg))?;
        if names.iter().take(i).any(|n| n == name) {
            return Err((i, format!("box credential {name:?} is listed twice")));
        }
    }
    Ok(())
}

/// A rule host: a domain or `*.domain`, no IPs, schemes or odd chars.
///
/// # Errors
/// Returns the reason text when the host is not a domain or `*.domain`.
pub fn host_entry(s: &str) -> Result<(), String> {
    let entry = s.trim().to_ascii_lowercase();
    let host = entry.strip_prefix("*.").unwrap_or(&entry);
    let bad = host.is_empty()
        || !host.contains('.')
        || host.contains(|c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == '-'))
        || host.parse::<std::net::IpAddr>().is_ok();
    if bad {
        return Err(format!("host must be a domain or *.domain: {s:?}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
