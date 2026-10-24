//! Both layers' `cache:` sections resolved into the volumes of one Box.

use std::fmt;

use super::{
    nested, overlap, presets, CacheConfig, CacheError, CacheName, Declared, Origin, Volume,
};

/// A declared volume left out of a plan because it met a sealed preset's directory, which stays
/// reserved even when that preset is turned off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dropped {
    /// Who declared the volume.
    pub origin: Origin,
    /// The volume left out.
    pub volume: CacheName,
    /// The sealed preset it met.
    pub preset: CacheName,
}

impl fmt::Display for Dropped {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let who = match self.origin {
            Origin::Operator => "operator",
            Origin::Preset | Origin::Repo => "repo",
        };
        write!(
            f,
            "{who} cache volume {} meets the sealed {} preset, which keeps its directory, so it \
             is dropped",
            self.volume.as_str(),
            self.preset.as_str()
        )
    }
}

/// Both layers' `cache:` sections for one Box.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CacheLayers {
    /// The repo's `.moor.yaml`.
    pub repo: CacheConfig,
    /// The operator's moor config.
    pub operator: CacheConfig,
}

/// The volumes of a Box, and the declared ones left out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// Every volume, presets first.
    pub volumes: Vec<Declared>,
    /// Declared volumes that met a sealed preset.
    pub dropped: Vec<Dropped>,
}

impl CacheLayers {
    /// Every volume of a Box, presets first; see [`CacheLayers::resolve`].
    ///
    /// # Errors
    /// As [`CacheLayers::resolve`].
    pub fn declared(&self, has_repo: bool) -> Result<Vec<Declared>, CacheError> {
        self.resolve(has_repo).map(|r| r.volumes)
    }

    /// Every volume of a Box, presets first, and the declared volumes left out. Presets and repo
    /// volumes need a repo. An unsealed preset that shares a name or a directory with a declared
    /// volume yields to it. A declared volume whose directory meets a sealed preset's is dropped
    /// instead, whether that preset is kept or not, so no unsealed volume ever holds it.
    ///
    /// # Errors
    /// A layer that does not check, or a repo and an operator volume that clash.
    pub fn resolve(&self, has_repo: bool) -> Result<Resolved, CacheError> {
        let invalid = |(at, why)| CacheError::Invalid(at, why);
        let repo = if has_repo {
            self.repo.check().map_err(invalid)?;
            self.repo.volumes().map_err(invalid)?
        } else {
            Vec::new()
        };
        self.operator.check().map_err(invalid)?;
        let operator = self.operator.volumes().map_err(invalid)?;
        let presets: Vec<Volume> = presets::all();
        let mut dropped = Vec::new();
        let declared: Vec<Declared> = (repo.into_iter().map(|v| (Origin::Repo, v)))
            .chain(operator.into_iter().map(|v| (Origin::Operator, v)))
            .filter_map(|(origin, volume)| {
                let sealed = presets
                    .iter()
                    .find(|p| !p.seal.is_empty() && nested(&volume, p));
                if let Some(p) = sealed {
                    dropped.push(Dropped {
                        origin,
                        volume: volume.name,
                        preset: p.name.clone(),
                    });
                    return None;
                }
                Some(Declared { origin, volume })
            })
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
        let kept = presets.into_iter().filter(|p| {
            has_repo
                && self.repo.presets.keeps(p.name.as_str())
                && self.operator.presets.keeps(p.name.as_str())
                && !declared.iter().any(|d| overlap(&d.volume, p).is_some())
        });
        let mut volumes: Vec<Declared> = kept
            .map(|volume| Declared {
                origin: Origin::Preset,
                volume,
            })
            .collect();
        volumes.extend(declared);
        Ok(Resolved { volumes, dropped })
    }
}
