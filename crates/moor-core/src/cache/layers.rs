//! Both layers' `cache:` sections resolved into the volumes of one Box.

use std::fmt;

use super::{
    agent, nested, overlap, presets, AgentCache, CacheConfig, CacheError, CacheName, Declared,
    Origin, Volume,
};
use crate::name::ForgeRepo;

/// A declared volume left out of a plan because it met the directory of a preset with restore steps, which stays
/// reserved even when that preset is turned off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dropped {
    /// Who declared the volume.
    pub origin: Origin,
    /// The volume left out.
    pub volume: CacheName,
    /// The preset it met.
    pub preset: CacheName,
}

impl fmt::Display for Dropped {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let who = match self.origin {
            Origin::Operator => "operator",
            Origin::Preset | Origin::Repo | Origin::Agent => "repo",
        };
        write!(
            f,
            "{who} cache volume {} meets the {} preset, which cleans its directory on restore, \
             so it is dropped",
            self.volume.as_str(),
            self.preset.as_str()
        )
    }
}

/// Both layers' `cache:` sections for one Box.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CacheLayers {
    /// The repo's config.
    pub repo: CacheConfig,
    /// The operator's moor config.
    pub operator: CacheConfig,
    /// The agent's install directory, when moor installs the agent in the Box.
    pub agent: Option<AgentCache>,
}

/// The volumes of a Box, and the declared ones left out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// Every volume, presets first.
    pub volumes: Vec<Declared>,
    /// Declared volumes that met a preset with restore steps.
    pub dropped: Vec<Dropped>,
}

impl CacheLayers {
    /// Every volume of a Box, presets first; see [`CacheLayers::resolve`].
    ///
    /// # Errors
    /// As [`CacheLayers::resolve`].
    pub fn declared(&self, repo: Option<&ForgeRepo>) -> Result<Vec<Declared>, CacheError> {
        self.resolve(repo).map(|r| r.volumes)
    }

    /// Every volume of a Box, presets first, and the declared volumes left out. Presets and repo
    /// volumes need a repo. A preset without restore steps that shares a name or a directory with a declared
    /// volume yields to it. A declared volume whose directory meets that of a preset with restore steps is dropped
    /// instead, whether that preset is kept or not, so no other volume ever holds it.
    ///
    /// # Errors
    /// A layer that does not check, or a repo and an operator volume that clash.
    pub fn resolve(&self, forge: Option<&ForgeRepo>) -> Result<Resolved, CacheError> {
        let has_repo = forge.is_some();
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
                let cleaned = presets
                    .iter()
                    .find(|p| !p.restore.is_empty() && nested(&volume, p));
                if let Some(p) = cleaned {
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
        volumes.extend(self.agent_volume(&declared));
        volumes.extend(declared);
        Ok(Resolved { volumes, dropped })
    }

    /// The environment the presets a Box of `forge` keeps need, so each tool finds its volume; none
    /// when the layers do not check.
    #[must_use]
    pub fn env(&self, forge: Option<&ForgeRepo>) -> Vec<(String, String)> {
        let Ok(resolved) = self.resolve(forge) else {
            return Vec::new();
        };
        resolved
            .volumes
            .iter()
            .filter(|d| d.origin == Origin::Preset)
            .flat_map(|d| presets::env(d.volume.name.as_str()))
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    /// The agent's install directory, unless it is not a cache path or a declared volume takes
    /// its name or a directory of it, to which it yields as a preset without restore steps would.
    fn agent_volume(&self, declared: &[Declared]) -> Option<Declared> {
        let volume = agent::volume(self.agent.as_ref()?)?;
        let taken = declared
            .iter()
            .any(|d| overlap(&d.volume, &volume).is_some());
        (!taken).then_some(Declared {
            origin: Origin::Agent,
            volume,
        })
    }
}
