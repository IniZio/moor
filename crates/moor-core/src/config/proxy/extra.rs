//! Extra `allow` entries a repo config or a Box override may ask for.
//!
//! Narrower than a user's `proxy.rules`: entries only join a default rule, each is
//! `METHODS /path` with a `{repo}` in the path, and nothing else of the rule changes.

use std::collections::BTreeMap;

use super::resolve::default_rules;
use super::APP_CREDENTIAL;
use crate::config::ConfigError;

/// The only slot an extra entry may use.
const REPO: &str = "{repo}";

/// Checks the `proxy.allow` map of a repo config: rule name to extra entries. The name
/// [`APP_CREDENTIAL`] takes entries that go out as the GitHub App, as `app` lines of the rule
/// for the GitHub API.
///
/// # Errors
/// [`ConfigError`] naming the entry that is not a plain `METHODS /path` on `{repo}` of an
/// existing default rule.
pub(in crate::config) fn check_extra_allow(
    extra: &BTreeMap<String, Vec<String>>,
) -> Result<(), ConfigError> {
    let defaults = default_rules();
    for (name, entries) in extra {
        let at = format!("proxy.allow.{name}");
        if !defaults.contains_key(name) && name != APP_CREDENTIAL {
            let mut known: Vec<&str> = defaults.keys().map(String::as_str).collect();
            known.push(APP_CREDENTIAL);
            return Err(ConfigError::new(
                at,
                format!(
                    "only a default rule can take extra entries ({}); a repo config cannot add hosts or credentials",
                    known.join(", ")
                ),
            ));
        }
        for (i, entry) in entries.iter().enumerate() {
            check_entry(entry).map_err(|w| ConfigError::new(format!("{at}[{i}]"), w))?;
        }
    }
    Ok(())
}

/// One entry: upper-case methods (no `*`), a path that names `{repo}` and no other slot or `**`.
fn check_entry(entry: &str) -> Result<(), String> {
    let bad = || {
        format!(
            "an extra entry must be \"METHOD /path/with/{{repo}}\", for example \"PATCH /repos/{{repo}}/issues/comments/*\", got {entry:?}"
        )
    };
    let (methods, path) = entry.split_once(' ').ok_or_else(bad)?;
    let method_ok = |m: &str| !m.is_empty() && m.chars().all(|c| c.is_ascii_uppercase());
    if !methods.split('|').all(method_ok)
        || !path.starts_with('/')
        || path.contains(char::is_whitespace)
    {
        return Err(bad());
    }
    if path.replace(REPO, "").contains(['{', '}']) {
        return Err(format!(
            "an extra entry may use {REPO} only, not {{any_repo}} or {{repo:urlenc}}, got {entry:?}"
        ));
    }
    if !path.contains(REPO) {
        return Err(format!(
            "an extra entry must name {REPO}, so it covers only the Box's own repo, got {entry:?}"
        ));
    }
    if path.contains("**") {
        return Err(format!(
            "an extra entry cannot use \"**\"; name the path, with \"*\" for one segment, got {entry:?}"
        ));
    }
    Ok(())
}
