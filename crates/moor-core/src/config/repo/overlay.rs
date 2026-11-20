//! A repo config laid over another for one Box: the override a start or a workflow carries.
//!
//! The override is written in the repo config's own YAML, so [`parse_repo`] checks it and no
//! second schema exists. What it *sets* is the top-level keys it names: a key it names replaces
//! the repo's value whole (lists such as `setup` and sections such as `cache`), except `env`,
//! which adds to the repo's variables by name, the override winning a clash, and `proxy`, whose
//! extra `allow` entries add to the repo's for the same rule. A key it leaves
//! out keeps the repo's value, so `setup: []` clears the setup lines and no `setup` leaves them.
//!
//! [`RepoOverride::over`] destructures [`RepoConfig`] in full, so a field added to it fails to
//! compile until it is given a rule there.

use std::collections::{BTreeMap, BTreeSet};

use serde::de::IgnoredAny;

use super::{parse_repo, RepoConfig};
use crate::config::{parse_strict, ConfigError};

/// `text` as an override request carries it: blank text is no override.
#[must_use]
pub fn override_text(text: Option<String>) -> Option<String> {
    text.filter(|t| !t.trim().is_empty())
}

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
        #[allow(
            clippy::zero_sized_map_values,
            reason = "the parser reads a map, and only its keys matter"
        )]
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
            proxy,
        } = self.config.clone();
        let mut out = base;
        self.replace("brief", brief, &mut out.brief);
        self.replace("setup", setup, &mut out.setup);
        self.replace("teardown", teardown, &mut out.teardown);
        self.merge("env", env, &mut out.env);
        self.replace("compose", compose, &mut out.compose);
        self.replace("box", box_, &mut out.box_);
        self.replace("box_credentials", box_credentials, &mut out.box_credentials);
        self.replace("cache", cache, &mut out.cache);
        self.replace("checkpoint", checkpoint, &mut out.checkpoint);
        self.replace("machine", machine, &mut out.machine);
        self.replace("node", node, &mut out.node);
        if self.set.contains("proxy") {
            for (rule, entries) in proxy.allow {
                let slot = out.proxy.allow.entry(rule).or_default();
                for entry in entries {
                    if !slot.contains(&entry) {
                        slot.push(entry);
                    }
                }
            }
        }
        out
    }

    /// Adds the entries of `more` to `slot`, each winning over the entry of its name, when the
    /// text named `key`.
    fn merge<V>(&self, key: &str, more: BTreeMap<String, V>, slot: &mut BTreeMap<String, V>) {
        if self.set.contains(key) {
            slot.extend(more);
        }
    }

    /// Puts `value` in `slot` when the text named `key`.
    fn replace<T>(&self, key: &str, value: T, slot: &mut T) {
        if self.set.contains(key) {
            *slot = value;
        }
    }
}

/// `text`, an override or none, with `vars` added to its `env`.
///
/// Each variable wins over one of its name. The other keys keep their values, so the result
/// overrides the repo's config as `text` did, plus the variables.
///
/// # Errors
/// [`ConfigError`] when `text` is not a valid override, or the result is refused.
pub fn with_env(text: Option<&str>, vars: &[(&str, String)]) -> Result<String, ConfigError> {
    const FILE: &str = "config";
    let text = text.unwrap_or_default();
    RepoOverride::parse(FILE, text)?;
    let read: serde_json::Value = serde_saphyr::from_str(text)
        .map_err(|e| ConfigError::new("", format!("invalid yaml: {e}")).in_file(FILE))?;
    let mut doc = match read {
        serde_json::Value::Object(map) => map,
        serde_json::Value::Null => serde_json::Map::new(),
        _ => return Err(ConfigError::new("", "expected mapping").in_file(FILE)),
    };
    let env = doc
        .entry("env")
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    if env.is_null() {
        *env = serde_json::Value::Object(serde_json::Map::new());
    }
    let serde_json::Value::Object(env) = env else {
        return Err(ConfigError::new("env", "expected mapping").in_file(FILE));
    };
    for (name, value) in vars {
        env.insert((*name).to_owned(), serde_json::Value::String(value.clone()));
    }
    let out = serde_saphyr::to_string(&doc).map_err(|e| {
        ConfigError::new("", format!("cannot write the override: {e}")).in_file(FILE)
    })?;
    RepoOverride::parse(FILE, &out)?;
    Ok(out)
}

#[cfg(test)]
mod tests;
