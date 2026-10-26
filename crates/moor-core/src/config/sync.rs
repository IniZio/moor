//! `machine.sync` in `.moor.yaml` and `machines.sync` in the user config: the rsync patterns a
//! sync to a user's machine leaves alone, so what the machine builds for itself survives.

use serde::Deserialize;

use super::ConfigError;
use crate::cache::Presets;

/// Most exclude patterns one list may hold.
pub const MAX_EXCLUDES: usize = 64;

/// The built-in presets. A pattern with no leading `/` matches at any depth.
const PRESETS: [(&str, &[&str]); 6] = [
    ("node", &["node_modules"]),
    ("cocoapods", &["Pods"]),
    ("gradle", &[".gradle"]),
    ("xcode", &["DerivedData", "*.xcworkspace/xcuserdata"]),
    ("dart", &[".dart_tool"]),
    ("rust", &["target"]),
];

/// One layer's `sync:` section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct SyncConfig {
    /// Which built-in presets this layer keeps.
    pub presets: Presets,
    /// rsync patterns this layer adds.
    pub exclude: Vec<String>,
}

impl SyncConfig {
    /// Checks the preset names and patterns; the error names the key below `at`.
    pub(super) fn validate(&self, at: &str) -> Result<(), ConfigError> {
        if let Presets::Drop(names) = &self.presets {
            for (i, n) in names.iter().enumerate() {
                if !PRESETS.iter().any(|(p, _)| p == n) {
                    let known: Vec<&str> = PRESETS.iter().map(|(p, _)| *p).collect();
                    let why = format!("no preset {n}; presets are {}", known.join(", "));
                    return Err(ConfigError::new(format!("{at}.presets[{i}]"), why));
                }
            }
        }
        if self.exclude.len() > MAX_EXCLUDES {
            let why = format!("at most {MAX_EXCLUDES} patterns");
            return Err(ConfigError::new(format!("{at}.exclude"), why));
        }
        for (i, p) in self.exclude.iter().enumerate() {
            check_exclude(p).map_err(|why| ConfigError::new(format!("{at}.exclude[{i}]"), why))?;
        }
        Ok(())
    }
}

/// The patterns a sync leaves alone: the presets both layers keep, then each layer's own.
#[must_use]
pub fn sync_excludes(repo: &SyncConfig, user: &SyncConfig) -> Vec<String> {
    let presets = PRESETS
        .iter()
        .filter(|(n, _)| repo.presets.keeps(n) && user.presets.keeps(n))
        .flat_map(|(_, ps)| ps.iter().map(|p| (*p).to_owned()));
    let mut out: Vec<String> = Vec::new();
    for p in presets
        .chain(repo.exclude.iter().cloned())
        .chain(user.exclude.iter().cloned())
    {
        if !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

/// Refuses a pattern rsync would read as an option or that cannot be one argument.
///
/// # Errors
/// An empty pattern, one starting with `-`, or one holding a newline or NUL.
pub fn check_exclude(pattern: &str) -> Result<(), String> {
    if pattern.is_empty() || pattern.starts_with('-') || pattern.contains(['\n', '\0']) {
        return Err(format!(
            "exclude pattern {pattern:?} is refused: it must not be empty, start with '-' or \
             hold a newline or NUL"
        ));
    }
    Ok(())
}

/// [`check_exclude`] on each of `patterns`, and at most [`MAX_EXCLUDES`] of them.
///
/// # Errors
/// Too many patterns, or the first bad one.
pub fn check_excludes(patterns: &[String]) -> Result<(), String> {
    if patterns.len() > MAX_EXCLUDES {
        return Err(format!(
            "exclude takes at most {MAX_EXCLUDES} patterns, not {}",
            patterns.len()
        ));
    }
    patterns.iter().try_for_each(|p| check_exclude(p))
}

#[cfg(test)]
mod tests;
