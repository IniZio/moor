//! Shared pure validators for user and repo config schemas.
//!
//! Each validator returns the one canonical message on failure; callers wrap it
//! in a `ConfigError` and do not reword it.

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

/// Box `gh` goes through the host proxy; `GH_TOKEN` may not be a credential.
///
/// # Errors
/// Returns the canonical message for `GH_TOKEN`.
pub fn not_gh_token(s: &str) -> Result<(), String> {
    if s == "GH_TOKEN" {
        Err(
            "GH_TOKEN is not supported: moor mints GitHub tokens from your GitHub App; \
             run `moor github setup` then `moor github login`"
                .to_owned(),
        )
    } else {
        Ok(())
    }
}

/// Claude variables a box agent gets through the host proxy, not credentials.
#[must_use]
pub fn proxied_claude_var(s: &str) -> bool {
    s == "CLAUDE_CODE_OAUTH_TOKEN" || s.starts_with("ANTHROPIC_")
}

/// Rejects the variables [`proxied_claude_var`] matches.
///
/// # Errors
/// Returns the canonical message for a proxied Claude variable.
pub fn not_proxied_claude_var(s: &str) -> Result<(), String> {
    if proxied_claude_var(s) {
        Err(format!(
            "{s} is not supported: the moor proxy supplies Claude credentials"
        ))
    } else {
        Ok(())
    }
}

/// Validates a box credential name list in order.
///
/// Each name must pass [`credential_name`], [`not_gh_token`] and
/// [`not_proxied_claude_var`], and may appear only once.
///
/// # Errors
/// Returns the index of the first failing name and its canonical message.
pub fn box_credentials(names: &[String]) -> Result<(), (usize, String)> {
    for (i, name) in names.iter().enumerate() {
        credential_name(name)
            .and_then(|()| not_gh_token(name))
            .and_then(|()| not_proxied_claude_var(name))
            .map_err(|msg| (i, msg))?;
        if names.iter().take(i).any(|n| n == name) {
            return Err((i, format!("box credential {name:?} is listed twice")));
        }
    }
    Ok(())
}

/// An egress entry: a domain or `*.domain`, no IPs, schemes or odd chars.
///
/// # Errors
/// Returns the reason text when the entry is not a domain or `*.domain`.
pub fn egress_entry(s: &str) -> Result<(), String> {
    let entry = s.trim().to_ascii_lowercase();
    let host = entry.strip_prefix("*.").unwrap_or(&entry);
    let bad = host.is_empty()
        || !host.contains('.')
        || host.contains(|c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == '-'))
        || host.parse::<std::net::IpAddr>().is_ok();
    if bad {
        return Err(format!("egress entry must be a domain or *.domain: {s:?}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
