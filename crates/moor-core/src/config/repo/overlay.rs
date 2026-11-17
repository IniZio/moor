//! A repo config laid over another for one Box: the override a start or a workflow carries.
//!
//! The override is written in the repo config's own YAML, so [`parse_repo`] checks it and no
//! second schema exists. What it *sets* is the top-level keys it names: a key it names replaces
//! the repo's value whole (lists such as `setup` and sections such as `cache`), except `env`,
//! which adds to the repo's variables by name, the override winning a clash. A key it leaves
//! out keeps the repo's value, so `setup: []` clears the setup lines and no `setup` leaves them.
//!
//! [`RepoOverride::over`] destructures [`RepoConfig`] in full, so a field added to it fails to
//! compile until it is given a rule there. A field that only adds (an allow list, say) takes the
//! union of both sides, as `env` does by key; every other field replaces.

use std::collections::{BTreeMap, BTreeSet};

use serde::de::IgnoredAny;

use super::{parse_repo, RepoConfig};
use crate::config::{parse_strict, ConfigError};

/// A repo config together with the top-level keys its text named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoOverride {
    /// The values the text gave; a key it left out holds the default and is not in `set`.
    config: RepoConfig,
    /// The top-level keys the text named, even with an empty value.
    set: BTreeSet<String>,
}

impl RepoOverride {
    /// Parses and validates `text` as a repo config that overrides another.
    ///
    /// # Errors
    /// [`ConfigError`] (with `file` set) for what [`parse_repo`] refuses.
    pub fn parse(file: &str, text: &str) -> Result<Self, ConfigError> {
        let config = parse_repo(file, text)?;
        let named: BTreeMap<String, IgnoredAny> = parse_strict(file, text)?;
        Ok(Self {
            config,
            set: named.into_keys().collect(),
        })
    }

    /// Whether the text named no key, so it changes nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.set.is_empty()
    }

    /// `base` with this override laid over it.
    #[must_use]
    pub fn over(&self, base: RepoConfig) -> RepoConfig {
        let RepoConfig {
            brief,
            setup,
            teardown,
            env,
            compose,
            box_,
            box_credentials,
            cache,
            checkpoint,
            machine,
            node,
        } = self.config.clone();
        let named = |key: &str| self.set.contains(key);
        let mut out = base;
        if named("brief") {
            out.brief = brief;
        }
        if named("setup") {
            out.setup = setup;
        }
        if named("teardown") {
            out.teardown = teardown;
        }
        if named("env") {
            out.env.extend(env);
        }
        if named("compose") {
            out.compose = compose;
        }
        if named("box") {
            out.box_ = box_;
        }
        if named("box_credentials") {
            out.box_credentials = box_credentials;
        }
        if named("cache") {
            out.cache = cache;
        }
        if named("checkpoint") {
            out.checkpoint = checkpoint;
        }
        if named("machine") {
            out.machine = machine;
        }
        if named("node") {
            out.node = node;
        }
        out
    }
}

#[cfg(test)]
mod tests;
