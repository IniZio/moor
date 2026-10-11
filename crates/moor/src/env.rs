//! Process environment and argv access; the only reader of either.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Snapshot of environment variables.
pub struct Env {
    /// Variable name to value.
    vars: BTreeMap<String, String>,
}

impl Env {
    /// Capture the process environment; non-UTF8 pairs are skipped.
    #[must_use]
    pub fn capture() -> Self {
        let vars = std::env::vars_os()
            .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
            .collect();
        Self { vars }
    }

    /// Build an environment from explicit pairs.
    pub fn from_pairs<I, K, V>(pairs: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        Self {
            vars: pairs
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        }
    }

    /// Raw process arguments, program name first.
    #[must_use]
    pub fn args() -> Vec<OsString> {
        std::env::args_os().collect()
    }

    /// Value of `key`, if set.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&str> {
        self.vars.get(key).map(String::as_str)
    }

    /// Override the state directory (sets `MOOR_STATE_DIR`).
    #[must_use]
    pub fn with_state_dir(mut self, dir: &Path) -> Self {
        self.vars.insert(
            "MOOR_STATE_DIR".to_owned(),
            dir.to_string_lossy().into_owned(),
        );
        self
    }

    /// Resolved state directory, or `None` when no source is set.
    #[must_use]
    pub fn state_dir(&self) -> Option<PathBuf> {
        if let Some(dir) = self.non_empty("MOOR_STATE_DIR") {
            return Some(PathBuf::from(dir));
        }
        if let Some(xdg) = self.non_empty("XDG_STATE_HOME") {
            return Some(Path::new(xdg).join("moor"));
        }
        let home = self.non_empty("HOME")?;
        Some(Path::new(home).join(".local/state/moor"))
    }

    /// Value of `key` when set and non-empty.
    fn non_empty(&self, key: &str) -> Option<&str> {
        self.get(key).filter(|v| !v.is_empty())
    }
}

#[cfg(test)]
mod tests;
