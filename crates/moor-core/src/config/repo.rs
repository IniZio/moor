//! The repo config file (`.moor.yaml`): strict parse plus validation, no IO.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use super::{check, parse_strict, BoxSize, ConfigError};

/// How a seed path lands in the worktree.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SeedMode {
    /// Copy the file or directory.
    #[default]
    Copy,
    /// Symlink to the host path.
    Symlink,
}

/// One `seed:` entry: a repo-relative path and how to place it.
///
/// Written as a bare string (copy) or `{path, mode}`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(from = "SeedEntry")]
pub struct Seed {
    /// Path relative to the repo root.
    pub path: PathBuf,
    /// Copy or symlink.
    pub mode: SeedMode,
}

/// Map form of a seed entry.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SeedFull {
    /// Path relative to the repo root.
    path: PathBuf,
    /// Copy unless set.
    #[serde(default)]
    mode: SeedMode,
}

/// Either spelling of a seed entry.
#[derive(Deserialize)]
#[serde(untagged, expecting = "a path string or a {path, mode} map")]
enum SeedEntry {
    /// Bare path, copied.
    Path(PathBuf),
    /// `{path, mode}` map.
    Full(SeedFull),
}

impl From<SeedEntry> for Seed {
    fn from(e: SeedEntry) -> Self {
        match e {
            SeedEntry::Path(path) => Self {
                path,
                mode: SeedMode::Copy,
            },
            SeedEntry::Full(f) => Self {
                path: f.path,
                mode: f.mode,
            },
        }
    }
}

/// `run:` section: environment for setup lines and `moor run`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RepoRun {
    /// Extra environment variables.
    pub env: BTreeMap<String, String>,
    /// Extra `PATH` entries.
    pub path: Vec<String>,
}

/// `compose:` section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RepoCompose {
    /// Prefix for compose project names.
    pub project_prefix: Option<String>,
}

/// `box:` section; only `size` is allowed at the repo layer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RepoBox {
    /// Box size class; repo > user > default.
    pub size: Option<BoxSize>,
}

/// The repo config. Build it with [`parse_repo`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RepoConfig {
    /// Setup commands.
    pub setup: Vec<String>,
    /// Teardown commands.
    pub teardown: Vec<String>,
    /// Extra egress domains (`domain` or `*.domain`).
    pub egress: Vec<String>,
    /// Plain environment variables for the Box.
    pub env: BTreeMap<String, String>,
    /// `compose:` section.
    pub compose: RepoCompose,
    /// `box:` section.
    #[serde(rename = "box")]
    pub box_: RepoBox,
    /// Names of user credentials exposed on the Box.
    pub box_credentials: Vec<String>,
    /// Repo paths placed in the worktree before the Box sync.
    pub seed: Vec<Seed>,
    /// `run:` section.
    pub run: RepoRun,
}

/// Parses and validates a `.moor.yaml` document.
///
/// # Errors
/// Returns [`ConfigError`] (with `file` set) for bad YAML, unknown
/// keys, and invalid values; the error path names the key.
pub fn parse_repo(file: &str, text: &str) -> Result<RepoConfig, ConfigError> {
    let cfg: RepoConfig = parse_strict(file, text)?;
    cfg.validate().map_err(|e| e.in_file(file))?;
    Ok(cfg)
}

impl RepoConfig {
    /// Checks values the types cannot.
    fn validate(&self) -> Result<(), ConfigError> {
        non_blank("setup", &self.setup)?;
        non_blank("teardown", &self.teardown)?;
        self.validate_box_credentials()?;
        for (i, e) in self.egress.iter().enumerate() {
            check::egress_entry(e).map_err(|r| ConfigError::new(format!("egress[{i}]"), r))?;
        }
        env_names("env", &self.env)?;
        env_names("run.env", &self.run.env)?;
        for (i, s) in self.seed.iter().enumerate() {
            if !is_inside_repo(&s.path) {
                return Err(ConfigError::new(
                    format!("seed[{i}]"),
                    format!(
                        "seed path must be relative inside the repo: {}",
                        s.path.display()
                    ),
                ));
            }
        }
        Ok(())
    }

    /// Checks each `box_credentials` name and rejects duplicates.
    fn validate_box_credentials(&self) -> Result<(), ConfigError> {
        check::box_credentials(&self.box_credentials)
            .map_err(|(i, m)| ConfigError::new(format!("box_credentials[{i}]"), m))
    }
}

/// Checks the names of an env map found under `section`.
fn env_names(section: &str, env: &BTreeMap<String, String>) -> Result<(), ConfigError> {
    for name in env.keys() {
        check::env_name(name)
            .and_then(|()| check::not_reserved_env(name))
            .map_err(|r| ConfigError::new(format!("{section}.{name}"), r))?;
    }
    Ok(())
}

/// True for a relative path that names something below the repo root.
///
/// Needs a normal component, so `.` and `./` (the root itself) fail, and
/// allows no `..`, root, or prefix components.
fn is_inside_repo(p: &Path) -> bool {
    p.components().any(|c| matches!(c, Component::Normal(_)))
        && p.components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

/// Rejects blank entries of a command list.
fn non_blank(name: &str, items: &[String]) -> Result<(), ConfigError> {
    items
        .iter()
        .position(|s| s.trim().is_empty())
        .map_or(Ok(()), |i| {
            Err(ConfigError::new(
                format!("{name}[{i}]"),
                format!("{name} entry must not be empty"),
            ))
        })
}

#[cfg(test)]
mod tests;
