//! Cache volumes as moor declares them: validated volumes, the scope each one is keyed to, and
//! the plan a Driver receives at acquire. The only place a cache key is composed.
//!
//! A volume declared by a repo, or a preset, is kept per user and repo; a volume the operator
//! declares is kept per user, across that user's repos. The scope follows who declared a volume,
//! so a repo cannot widen its own.

mod config;
mod layers;
mod path;
pub mod presets;
mod seal;
#[cfg(test)]
mod tests;

use std::fmt::{self, Write as _};
use std::num::NonZeroU32;

use hmac::digest::Key;
use hmac::{Hmac, Mac};
use sha2::Sha256;

pub use config::{CacheConfig, Presets, VolumeDecl};
pub use layers::{CacheLayers, Dropped, Resolved};
pub use path::{owned_paths, CachePath};
pub use seal::{failed as seal_failed, SEAL_FAILED};

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

/// Whether any path of `a` equals, holds or sits inside any path of `b`.
fn nested(a: &Volume, b: &Volume) -> bool {
    a.paths
        .iter()
        .any(|p| b.paths.iter().any(|q| p.overlaps(q)))
}

/// Why `a` and `b` cannot share a plan, if they cannot.
fn overlap(a: &Volume, b: &Volume) -> Option<CacheError> {
    if a.name == b.name {
        return Some(CacheError::Duplicate(a.name.0.clone()));
    }
    nested(a, b).then(|| CacheError::Overlap(a.name.0.clone(), b.name.0.clone()))
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
    /// The scope id, with `x` appended for a volume with a seal so that it is kept alone; the
    /// volumes of one group share one store.
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
pub struct CachePlan {
    /// The volumes.
    volumes: Vec<PlannedVolume>,
    /// Declared volumes left out.
    dropped: Vec<Dropped>,
    /// The repo-scope hash, the Box's own scope; set only for a repo Box with volumes.
    scope: Option<String>,
}

impl CachePlan {
    /// No volumes.
    #[must_use]
    pub const fn none() -> Self {
        Self {
            volumes: Vec::new(),
            dropped: Vec::new(),
            scope: None,
        }
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
        let Resolved { volumes, dropped } = layers.resolve(repo.is_some())?;
        let planned = volumes.into_iter().map(|d| {
            let mut group = group(salt, d.origin, user, repo);
            if !d.volume.seal.is_empty() {
                group.push('x');
            }
            PlannedVolume {
                key: format!("{group}-{}", d.volume.name.as_str()),
                group,
                name: d.volume.name,
                paths: d.volume.paths,
                size_gb: d.volume.size_gb,
                seal: d.volume.seal,
            }
        });
        let volumes: Vec<PlannedVolume> = planned.collect();
        let scope = repo
            .filter(|_| !volumes.is_empty())
            .map(|_| group(salt, Origin::Repo, user, repo));
        Ok(Self {
            volumes,
            dropped,
            scope,
        })
    }

    /// The volumes.
    #[must_use]
    pub fn volumes(&self) -> &[PlannedVolume] {
        &self.volumes
    }

    /// The scope of the Box itself: the repo group's hash without the sealed suffix. None for a
    /// Box without a repo or without volumes.
    #[must_use]
    pub fn scope(&self) -> Option<&str> {
        self.scope.as_deref()
    }

    /// Declared volumes left out because they met a sealed preset.
    #[must_use]
    pub fn dropped(&self) -> &[Dropped] {
        &self.dropped
    }

    /// Whether the Box starts cold.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.volumes.is_empty()
    }

    /// The script to run in the Box at close, after teardown; none when no volume has seal steps.
    /// A volume whose seal fails is emptied and named on stdout (see [`seal_failed`]).
    #[must_use]
    pub fn seal_script(&self) -> Option<String> {
        seal::script(&self.volumes)
    }
}
