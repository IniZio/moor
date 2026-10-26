//! The `cache:` section, in `.moor.yaml` and in the operator's moor config.

use std::num::NonZeroU32;

use serde::Deserialize;

use super::{overlap, presets, CacheName, CachePath, OnFail, Volume, WorkTree};

/// Most volumes one layer may declare.
const MAX_VOLUMES: usize = 8;

/// `cache.presets`: `true` (the default), `false`, or the names of presets to drop.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum Presets {
    /// All presets on, or all off.
    All(bool),
    /// All presets except these.
    Drop(Vec<String>),
}

impl Default for Presets {
    fn default() -> Self {
        Self::All(true)
    }
}

impl Presets {
    /// Whether this layer keeps the preset `name`.
    pub(crate) fn keeps(&self, name: &str) -> bool {
        match self {
            Self::All(on) => *on,
            Self::Drop(names) => !names.iter().any(|n| n == name),
        }
    }
}

/// One `cache.volumes` entry as written.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VolumeDecl {
    /// `[a-z][a-z0-9-]{0,31}`.
    pub name: String,
    /// The directory kept, in a form [`CachePath::parse`] takes.
    pub path: String,
    /// Size hint in GB; the provider's default when unset.
    #[serde(default)]
    pub size_gb: Option<NonZeroU32>,
    /// Shell steps run in the Box at close, before the volume is kept.
    #[serde(default)]
    pub seal: Vec<String>,
}

/// The `cache:` section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct CacheConfig {
    /// Which built-in presets this layer keeps.
    pub presets: Presets,
    /// Volumes this layer declares.
    pub volumes: Vec<VolumeDecl>,
    /// Whether this layer keeps the repo's work tree, and its size.
    pub work_tree: WorkTree,
}

impl CacheConfig {
    /// Checks every value; the error names the key below `cache` and why.
    ///
    /// # Errors
    /// An unknown preset name, too many volumes, an invalid name or path, a blank seal step,
    /// or two volumes with one name or overlapping paths.
    pub fn check(&self) -> Result<(), (String, String)> {
        if let Presets::Drop(names) = &self.presets {
            let known = presets::names();
            if let Some((i, n)) = names
                .iter()
                .enumerate()
                .find(|(_, n)| !known.contains(&n.as_str()))
            {
                let why = format!("no preset {n}; presets are {}", known.join(", "));
                return Err((format!("presets[{i}]"), why));
            }
        }
        let volumes = self.volumes()?;
        for (i, v) in volumes.iter().enumerate() {
            if let Some(clash) = volumes.iter().take(i).find_map(|w| overlap(w, v)) {
                return Err((format!("volumes[{i}]"), clash.to_string()));
            }
        }
        Ok(())
    }

    /// The declared volumes, validated one by one.
    pub(super) fn volumes(&self) -> Result<Vec<Volume>, (String, String)> {
        if self.volumes.len() > MAX_VOLUMES {
            let why = format!("at most {MAX_VOLUMES} volumes");
            return Err(("volumes".to_owned(), why));
        }
        let at = |i: usize, key: &str| format!("volumes[{i}].{key}");
        self.volumes
            .iter()
            .enumerate()
            .map(|(i, d)| {
                let name = CacheName::parse(&d.name).map_err(|e| (at(i, "name"), e.to_string()))?;
                let path = CachePath::parse(&d.path).map_err(|e| (at(i, "path"), e.to_string()))?;
                if let Some(s) = d.seal.iter().position(|s| s.trim().is_empty()) {
                    let why = "seal entry must not be empty".to_owned();
                    return Err((format!("volumes[{i}].seal[{s}]"), why));
                }
                Ok(Volume {
                    name,
                    paths: vec![path],
                    size_gb: d.size_gb,
                    seal: d.seal.clone(),
                    on_fail: OnFail::Clear,
                })
            })
            .collect()
    }
}
