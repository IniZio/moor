//! Owner-configured toolchains on the Box host, and the repo's selection of them.
//!
//! A repo config names toolchains; it can never add a root or widen one.

use std::collections::BTreeMap;

use serde::Deserialize;

use super::super::{check, ConfigError};

/// Environment names a toolchain may not set.
const RESERVED_ENV: [&str; 6] = [
    "HOME",
    "TMPDIR",
    "CFFIXED_USER_HOME",
    "XCODE_XCCONFIG_FILE",
    "MOOR_BOX_TAG",
    "DEVELOPER_DIR",
];

/// One owner-configured toolchain on the Box host.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostToolchain {
    /// Absolute directory holding the toolchain.
    pub root: String,
    /// Environment to export; `{root}` in a value expands to `root`.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Directories for `PATH`, relative to `root`.
    #[serde(default)]
    pub bin: Vec<String>,
}

/// Toolchains by name.
pub type ToolchainTable = BTreeMap<String, HostToolchain>;

/// A toolchain a repo selected, with `{root}` expanded and bin dirs made absolute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedToolchain {
    /// Name in the host config.
    pub name: String,
    /// Absolute root.
    pub root: String,
    /// Environment sorted by name, `{root}` expanded.
    pub env: Vec<(String, String)>,
    /// Absolute `PATH` directories.
    pub bin: Vec<String>,
}

/// Checks every toolchain in the table.
///
/// # Errors
/// Returns the first invalid name, root, env entry or bin entry.
pub(super) fn validate_toolchains(table: &ToolchainTable) -> Result<(), ConfigError> {
    for (name, t) in table {
        let at = format!("toolchains.{name}");
        let named = !name.is_empty()
            && name.chars().all(|c| {
                c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '.' | '_')
            });
        if !named {
            return Err(ConfigError::new(
                at,
                "name must be lowercase letters, digits, '-', '.' or '_'",
            ));
        }
        root_ok(&t.root).map_err(|m| ConfigError::new(format!("{at}.root"), m))?;
        for k in t.env.keys() {
            env_ok(k).map_err(|m| ConfigError::new(format!("{at}.env.{k}"), m))?;
        }
        for (i, b) in t.bin.iter().enumerate() {
            bin_ok(b).map_err(|m| ConfigError::new(format!("{at}.bin[{i}]"), m))?;
        }
    }
    Ok(())
}

/// Refuses roots that are not a plain absolute path.
fn root_ok(r: &str) -> Result<(), String> {
    if !r.starts_with('/') {
        return Err("root must be an absolute path".into());
    }
    if r == "/" {
        return Err("root must not be /".into());
    }
    if r.ends_with('/') {
        return Err("root must not end with /".into());
    }
    if r.split('/').any(|c| c == "." || c == "..") {
        return Err("root must not contain . or .. components".into());
    }
    if r.chars().any(|c| c == '"' || c == '\\' || c.is_control()) {
        return Err("root must not contain quotes, backslashes or control characters".into());
    }
    Ok(())
}

/// Refuses names that are not shell identifiers or that moor owns.
fn env_ok(k: &str) -> Result<(), String> {
    check::env_name(k)?;
    if k == "PATH" {
        return Err("PATH is reserved; PATH entries go in bin".into());
    }
    if RESERVED_ENV.contains(&k) {
        return Err(format!("name {k:?} is reserved"));
    }
    Ok(())
}

/// Refuses bin entries that are not a plain path under the root.
fn bin_ok(b: &str) -> Result<(), String> {
    if b.is_empty() {
        return Err("bin entry must not be empty".into());
    }
    if b.starts_with('/') {
        return Err("bin entry is relative to root and must not start with /".into());
    }
    if b.split('/').any(|c| c == "..") {
        return Err("bin entry must not contain .. components".into());
    }
    Ok(())
}

/// Resolves the toolchains a repo named, in the order named.
///
/// # Errors
/// Returns a [`ConfigError`] at `box.toolchains[<i>]` for a name the host
/// config does not define.
pub fn select_toolchains(
    table: &ToolchainTable,
    names: &[String],
) -> Result<Vec<SelectedToolchain>, ConfigError> {
    names
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let t = table.get(name).ok_or_else(|| {
                let known: Vec<&str> = table.keys().map(String::as_str).collect();
                let known = if known.is_empty() {
                    " none".to_string()
                } else {
                    format!(": {}", known.join(", "))
                };
                ConfigError::new(
                    format!("box.toolchains[{i}]"),
                    format!("unknown toolchain \"{name}\"; the host config names{known}"),
                )
            })?;
            Ok(SelectedToolchain {
                name: name.clone(),
                root: t.root.clone(),
                env: t
                    .env
                    .iter()
                    .map(|(k, v)| (k.clone(), v.replace("{root}", &t.root)))
                    .collect(),
                bin: t.bin.iter().map(|b| format!("{}/{b}", t.root)).collect(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests;
