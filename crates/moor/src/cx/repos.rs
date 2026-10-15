//! Controller-known repos on a context.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use moor_core::config::{parse_repo, MoorYamlSource, RepoConfig, RepoTable};
use moor_core::ErrorKind;

use super::Cx;
use crate::error::CommandError;

impl Cx {
    /// This context knowing `repos`.
    pub(crate) fn with_repos(mut self, repos: RepoTable) -> Self {
        self.repos = Arc::new(repos);
        self
    }

    /// Repos the controller knows.
    pub(crate) fn known_repos(&self) -> &RepoTable {
        &self.repos
    }

    /// Cache checkout of the known repo `name`: `<state_dir>/repos/<name>`.
    pub(crate) fn repo_cache(&self, name: &str) -> Result<PathBuf, CommandError> {
        Ok(self.state_dir()?.join("repos").join(name))
    }

    /// Label and text of the `.moor.yaml` the controller supplies for `main_checkout`.
    pub(crate) fn repo_overlay(
        &self,
        main_checkout: &Path,
    ) -> Result<Option<(String, String)>, CommandError> {
        if self.state_dir.is_none() {
            return Ok(None);
        }
        let want = std::fs::canonicalize(main_checkout).ok();
        for (name, entry) in self.repos.iter() {
            let Some(source) = &entry.moor_yaml else {
                continue;
            };
            let cache = self.repo_cache(name)?;
            let same = cache == main_checkout
                || matches!((&want, std::fs::canonicalize(&cache)), (Some(w), Ok(c)) if *w == c);
            if !same {
                continue;
            }
            return match source {
                MoorYamlSource::Inline(text) => {
                    Ok(Some((format!("controller repo {name}"), text.clone())))
                }
                MoorYamlSource::Path(path) => std::fs::read_to_string(path)
                    .map(|text| Some((path.clone(), text)))
                    .map_err(|e| CommandError::new(ErrorKind::Failed, format!("read {path}: {e}"))),
            };
        }
        Ok(None)
    }

    /// Parsed overlay for `main_checkout`.
    pub(super) fn overlay_config(
        &self,
        main_checkout: &Path,
    ) -> Result<Option<RepoConfig>, CommandError> {
        match self.repo_overlay(main_checkout)? {
            Some((label, text)) => Ok(Some(parse_repo(&label, &text)?)),
            None => Ok(None),
        }
    }
}
