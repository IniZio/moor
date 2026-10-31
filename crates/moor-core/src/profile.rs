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

/// The marketplace name that stands for the profile repo itself. The manifest cannot see the
/// checkout, so it never declares this one; the writers point it at the checkout.
pub const PROFILE_MARKETPLACE: &str = "profile";

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
    /// Environment variables the agent process starts with: literal values, where `{home}` and
    /// `{profile}` stand for the Box user's home and the profile checkout.
    pub env: BTreeMap<String, String>,
}

/// The `codex:` section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct CodexSection {
    /// Model passed to Codex.
    pub model: Option<String>,
    /// Marketplace name to its GitHub repo.
    pub marketplaces: BTreeMap<String, GithubRepo>,
    /// Plugins to install; each names a declared marketplace.
    pub plugins: Vec<PluginRef>,
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
    check_plugins(
        "claude",
        &manifest.claude.marketplaces,
        &manifest.claude.plugins,
    )?;
    check_plugins(
        "codex",
        &manifest.codex.marketplaces,
        &manifest.codex.plugins,
    )?;
    Ok(manifest)
}

/// Refuses a plugin in `section` that names a marketplace neither declared nor the profile's own.
fn check_plugins(
    section: &str,
    marketplaces: &BTreeMap<String, GithubRepo>,
    plugins: &[PluginRef],
) -> Result<(), ProfileError> {
    let fail = |path: String, msg: String| ConfigError::new(path, msg).in_file(MANIFEST_FILE);
    if marketplaces.contains_key(PROFILE_MARKETPLACE) {
        return Err(fail(
            format!("{section}.marketplaces.{PROFILE_MARKETPLACE}"),
            "`profile` is reserved for the profile repo itself".to_owned(),
        ));
    }
    for (i, plugin) in plugins.iter().enumerate() {
        if plugin.marketplace != PROFILE_MARKETPLACE
            && !marketplaces.contains_key(&plugin.marketplace)
        {
            return Err(fail(
                format!("{section}.plugins[{i}]"),
                format!("marketplace `{}` is not declared", plugin.marketplace),
            ));
        }
    }
    Ok(())
}

/// The marketplace repos the Proxy must allow for this manifest, each once: Claude's in name order,
/// then Codex's. The `profile` marketplace is the profile repo, which the Proxy already allows.
#[must_use]
pub fn repos(manifest: &Manifest) -> Vec<ForgeRepo> {
    let mut out: Vec<ForgeRepo> = Vec::new();
    let all = manifest
        .claude
        .marketplaces
        .values()
        .chain(manifest.codex.marketplaces.values());
    for GithubRepo(repo) in all {
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

/// Where a marketplace repo lists its plugins.
pub const MARKETPLACE_FILE: &str = ".claude-plugin/marketplace.json";

/// The GitHub repos the plugins named `wanted` in the marketplace file of manifest key `key` are
/// sourced from, each once. A plugin in the marketplace's own repo, or on another host, adds
/// nothing.
///
/// # Errors
/// Why, when `json` is not a marketplace file or names itself other than `key`, since plugins
/// install as `plugin@<its name>`.
pub fn plugin_sources(json: &str, key: &str, wanted: &[&str]) -> Result<Vec<ForgeRepo>, String> {
    #[derive(Deserialize)]
    struct Marketplace {
        name: String,
        plugins: Vec<Plugin>,
    }
    #[derive(Deserialize)]
    struct Plugin {
        name: String,
        source: serde_json::Value,
    }
    let market: Marketplace =
        serde_json::from_str(json).map_err(|e| format!("marketplace {key}: {e}"))?;
    if market.name != key {
        return Err(format!("marketplace {key} is named {}", market.name));
    }
    let mut out: Vec<ForgeRepo> = Vec::new();
    for plugin in market
        .plugins
        .iter()
        .filter(|p| wanted.contains(&p.name.as_str()))
    {
        let source = &plugin.source;
        let repo = match source.get("source").and_then(serde_json::Value::as_str) {
            Some("github") => source
                .get("repo")
                .and_then(serde_json::Value::as_str)
                .and_then(|r| ForgeRepo::parse(r).ok()),
            Some("url" | "git-subdir") => source
                .get("url")
                .and_then(serde_json::Value::as_str)
                .and_then(ForgeRepo::from_github_url),
            _ => None,
        };
        if let Some(repo) = repo.filter(|r| !out.contains(r)) {
            out.push(repo);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
