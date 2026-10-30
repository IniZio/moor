//! The person's agent profile manifest (`moor-profile.yaml`): strict parse plus validation, no IO.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::config::{parse_strict, ConfigError};
use crate::name::ForgeRepo;

/// Why a manifest was refused; the path names the offending key.
pub type ProfileError = ConfigError;

/// The file name of the manifest at the root of a profile repo.
pub const MANIFEST_FILE: &str = "moor-profile.yaml";

/// A GitHub repo written as `https://github.com/<owner>/<repo>`, with an optional `.git`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct GithubRepo(pub ForgeRepo);

impl TryFrom<String> for GithubRepo {
    type Error = String;

    fn try_from(url: String) -> Result<Self, Self::Error> {
        let path = url
            .strip_prefix("https://github.com/")
            .ok_or("must be https://github.com/<owner>/<repo>")?;
        let path = path.strip_suffix(".git").unwrap_or(path);
        ForgeRepo::parse(path)
            .map(Self)
            .map_err(|_| "must be https://github.com/<owner>/<repo>".to_owned())
    }
}

/// A plugin written as `<name>@<marketplace>`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct PluginRef {
    /// The plugin's name inside its marketplace.
    pub name: String,
    /// The key of the marketplace in the same agent section.
    pub marketplace: String,
}

impl TryFrom<String> for PluginRef {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        match s.split_once('@') {
            Some((name, marketplace))
                if !name.is_empty() && !marketplace.is_empty() && !marketplace.contains('@') =>
            {
                Ok(Self {
                    name: name.to_owned(),
                    marketplace: marketplace.to_owned(),
                })
            }
            _ => Err("must be <plugin>@<marketplace>".to_owned()),
        }
    }
}

/// The `claude:` section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ClaudeSection {
    /// Model passed to Claude Code.
    pub model: Option<String>,
    /// Marketplace name to its GitHub repo.
    pub marketplaces: BTreeMap<String, GithubRepo>,
    /// Plugins to install; each names a declared marketplace.
    pub plugins: Vec<PluginRef>,
}

/// The `codex:` section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct CodexSection {
    /// Model passed to Codex.
    pub model: Option<String>,
}

/// The parsed `moor-profile.yaml`. Build it with [`parse`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Manifest {
    /// `claude:` section.
    pub claude: ClaudeSection,
    /// `codex:` section.
    pub codex: CodexSection,
}

/// Parses and validates a `moor-profile.yaml` document. An empty document is an empty manifest.
///
/// # Errors
/// [`ProfileError`] for bad YAML, unknown keys, a marketplace that is not a GitHub repo URL, a
/// plugin without a marketplace, or a plugin naming a marketplace the file does not declare.
pub fn parse(text: &str) -> Result<Manifest, ProfileError> {
    let manifest: Manifest = parse_strict(MANIFEST_FILE, text)?;
    for (i, plugin) in manifest.claude.plugins.iter().enumerate() {
        if !manifest
            .claude
            .marketplaces
            .contains_key(&plugin.marketplace)
        {
            return Err(ConfigError::new(
                format!("claude.plugins[{i}]"),
                format!("marketplace `{}` is not declared", plugin.marketplace),
            )
            .in_file(MANIFEST_FILE));
        }
    }
    Ok(manifest)
}

/// The marketplace repos the Proxy must allow for this manifest, each once, in name order.
#[must_use]
pub fn repos(manifest: &Manifest) -> Vec<ForgeRepo> {
    let mut out: Vec<ForgeRepo> = Vec::new();
    for GithubRepo(repo) in manifest.claude.marketplaces.values() {
        if !out.contains(repo) {
            out.push(repo.clone());
        }
    }
    out
}

/// The model for one agent: the flag, else the profile's, else the user config's.
#[must_use]
pub fn model(flag: Option<&str>, profile: Option<&str>, config: Option<&str>) -> Option<String> {
    flag.or(profile).or(config).map(str::to_owned)
}

#[cfg(test)]
mod tests;
