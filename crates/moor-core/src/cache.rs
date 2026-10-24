//! Cache volumes as moor declares them: validated volumes, the scope each one is keyed to, and
//! the plan a Driver receives at acquire. The only place a cache key is composed.
//!
//! A volume declared by a repo, or a preset, is kept per user and repo; a volume the operator
//! declares is kept per user, across that user's repos. The scope follows who declared a volume,
//! so a repo cannot widen its own.

mod config;
mod path;
pub mod presets;
#[cfg(test)]
mod tests;

use std::fmt::{self, Write as _};
use std::num::NonZeroU32;

use hmac::digest::Key;
use hmac::{Hmac, Mac};
use sha2::Sha256;

pub use config::{CacheConfig, Presets, VolumeDecl};
pub use path::{owned_paths, CachePath};

use crate::name::ForgeRepo;
use crate::user::UserId;

/// A volume name, `[a-z][a-z0-9-]{0,31}`; a subdirectory of its scope's store.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct CacheName(String);

impl CacheName {
    /// Validates `s`.
    ///
    /// # Errors
    /// [`CacheError::Name`] when `s` breaks the pattern.
    pub fn parse(s: &str) -> Result<Self, CacheError> {
        let mut bytes = s.bytes();
        let ok = s.len() <= 32
            && bytes.next().is_some_and(|b| b.is_ascii_lowercase())
            && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        ok.then(|| Self(s.to_owned()))
            .ok_or_else(|| CacheError::Name(s.to_owned()))
    }

    /// The name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One volume: a name over one or more paths, built only by validation or the preset table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Volume {
    /// Name.
    name: CacheName,
    /// Directories kept, never overlapping.
    paths: Vec<CachePath>,
    /// Size hint in GB.
    size_gb: Option<NonZeroU32>,
    /// Shell steps run in the Box at close.
    seal: Vec<String>,
}

impl Volume {
    /// The name.
    #[must_use]
    pub const fn name(&self) -> &CacheName {
        &self.name
    }

    /// The directories kept.
    #[must_use]
    pub fn paths(&self) -> &[CachePath] {
        &self.paths
    }
}

/// Who declared a volume, which alone decides its scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// A built-in preset: scoped like a repo volume.
    Preset,
    /// The repo's `.moor.yaml`: kept per user and repo.
    Repo,
    /// The operator's moor config: kept per user, across repos.
    Operator,
}

/// A volume and who declared it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declared {
    /// Who declared it.
    pub origin: Origin,
    /// The volume.
    pub volume: Volume,
}

/// Why two volumes cannot share a plan.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CacheError {
    /// A name breaking `[a-z][a-z0-9-]{0,31}`.
    #[error("cache volume name {0:?} must match [a-z][a-z0-9-]{{0,31}}")]
    Name(String),
    /// A path moor will not cache.
    #[error("cache path {path}: {why}")]
    Path {
        /// The path as written.
        path: String,
        /// Why it is refused.
        why: String,
    },
    /// Two volumes with one name.
    #[error("two cache volumes are named {0}")]
    Duplicate(String),
    /// Two volumes whose paths are equal or nested.
    #[error("cache volumes {0} and {1} cover the same directory")]
    Overlap(String, String),
    /// A layer whose `cache:` section does not check, named by its key.
    #[error("cache.{0}: {1}")]
    Invalid(String, String),
}

/// Why `a` and `b` cannot share a plan, if they cannot.
fn overlap(a: &Volume, b: &Volume) -> Option<CacheError> {
    if a.name == b.name {
        return Some(CacheError::Duplicate(a.name.0.clone()));
    }
    let nested = a
        .paths
        .iter()
        .any(|p| b.paths.iter().any(|q| p.overlaps(q)));
    nested.then(|| CacheError::Overlap(a.name.0.clone(), b.name.0.clone()))
}

/// Both layers' `cache:` sections for one Box.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CacheLayers {
    /// The repo's `.moor.yaml`.
    pub repo: CacheConfig,
    /// The operator's moor config.
    pub operator: CacheConfig,
}

impl CacheLayers {
    /// Every volume of a Box, presets first. Presets and repo volumes need a repo. A preset that
    /// shares a name or a directory with a declared volume yields to it.
    ///
    /// # Errors
    /// A layer that does not check, or a repo and an operator volume that clash.
    pub fn declared(&self, has_repo: bool) -> Result<Vec<Declared>, CacheError> {
        let invalid = |(at, why)| CacheError::Invalid(at, why);
        let repo = if has_repo {
            self.repo.check().map_err(invalid)?;
            self.repo.volumes().map_err(invalid)?
        } else {
            Vec::new()
        };
        self.operator.check().map_err(invalid)?;
        let operator = self.operator.volumes().map_err(invalid)?;
        let declared: Vec<Declared> = (repo.into_iter().map(|v| (Origin::Repo, v)))
            .chain(operator.into_iter().map(|v| (Origin::Operator, v)))
            .map(|(origin, volume)| Declared { origin, volume })
            .collect();
        for (i, d) in declared.iter().enumerate() {
            if let Some(e) = declared
                .iter()
                .take(i)
                .find_map(|e| overlap(&e.volume, &d.volume))
            {
                return Err(e);
            }
        }
        let kept = presets::all().into_iter().filter(|p| {
            let on = self.repo.presets.keeps(p.name.as_str())
                && self.operator.presets.keeps(p.name.as_str());
            has_repo && on && !declared.iter().any(|d| overlap(&d.volume, p).is_some())
        });
        let mut all: Vec<Declared> = kept
            .map(|volume| Declared {
                origin: Origin::Preset,
                volume,
            })
            .collect();
        all.extend(declared);
        Ok(all)
    }
}

/// Random per state dir; a fresh state dir orphans every cache rather than handing it to
/// whoever gets the same user id there.
#[derive(Clone, PartialEq, Eq)]
pub struct CacheSalt([u8; 64]);

impl CacheSalt {
    /// Wraps random bytes.
    #[must_use]
    pub const fn new(bytes: [u8; 64]) -> Self {
        Self(bytes)
    }

    /// Lowercase hex, as stored.
    #[must_use]
    pub fn to_hex(&self) -> String {
        hex(&self.0)
    }

    /// Parses [`CacheSalt::to_hex`] output.
    #[must_use]
    pub fn from_hex(s: &str) -> Option<Self> {
        let mut out = [0u8; 64];
        let digits = s.as_bytes().chunks(2);
        if s.len() != 128 {
            return None;
        }
        for (b, pair) in out.iter_mut().zip(digits) {
            *b = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
        }
        Some(Self(out))
    }
}

impl fmt::Debug for CacheSalt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CacheSalt(..)")
    }
}

/// Lowercase hex of `bytes`.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

/// The 16-hex scope id of `origin` for `user` and `repo`: one store per scope.
fn group(salt: &CacheSalt, origin: Origin, user: UserId, repo: Option<&ForgeRepo>) -> String {
    let repo = repo.map_or_else(String::new, |r| {
        format!("{}/{}", r.owner.to_lowercase(), r.name.to_lowercase())
    });
    let scope = match origin {
        Origin::Preset | Origin::Repo => format!("repo\0{}\0{repo}", user.get()),
        Origin::Operator => format!("user\0{}", user.get()),
    };
    let mut mac = Hmac::<Sha256>::new(Key::<Hmac<Sha256>>::from_slice(&salt.0));
    mac.update(scope.as_bytes());
    let digest = mac.finalize().into_bytes();
    hex(digest.get(..8).unwrap_or_default())
}

/// One volume as the Driver receives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedVolume {
    /// The scope id; the volumes of one group share one store.
    pub group: String,
    /// Name, unique in the plan.
    pub name: CacheName,
    /// `<group>-<name>`.
    pub key: String,
    /// Directories kept.
    pub paths: Vec<CachePath>,
    /// Size hint in GB; the provider's default when unset.
    pub size_gb: Option<NonZeroU32>,
    /// Shell steps run in the Box at close.
    seal: Vec<String>,
}

impl PlannedVolume {
    /// Shell steps run in the Box at close, in order.
    #[must_use]
    pub fn seal(&self) -> &[String] {
        &self.seal
    }
}

/// What a Driver receives at acquire; empty means a cold Box.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CachePlan(Vec<PlannedVolume>);

impl CachePlan {
    /// No volumes.
    #[must_use]
    pub const fn none() -> Self {
        Self(Vec::new())
    }

    /// Keys every volume of `layers` to its scope for `user` and `repo`; a Box without a repo
    /// gets the operator's volumes only.
    ///
    /// # Errors
    /// As [`CacheLayers::declared`].
    pub fn build(
        user: UserId,
        repo: Option<&ForgeRepo>,
        layers: &CacheLayers,
        salt: &CacheSalt,
    ) -> Result<Self, CacheError> {
        let planned = layers.declared(repo.is_some())?.into_iter().map(|d| {
            let group = group(salt, d.origin, user, repo);
            PlannedVolume {
                key: format!("{group}-{}", d.volume.name.as_str()),
                group,
                name: d.volume.name,
                paths: d.volume.paths,
                size_gb: d.volume.size_gb,
                seal: d.volume.seal,
            }
        });
        Ok(Self(planned.collect()))
    }

    /// The volumes.
    #[must_use]
    pub fn volumes(&self) -> &[PlannedVolume] {
        &self.0
    }

    /// Whether the Box starts cold.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}
