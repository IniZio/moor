//! Per-command settings resolved once from flags, repo and user config.
//!
//! Precedence is flags > repo > user > default, where a field is allowed at
//! that layer. Each resolved field keeps the layer it came from so `env` and
//! the credential preflight need not re-read any file. Pure: no IO.

use std::collections::{BTreeMap, BTreeSet};

use super::{BoxSize, DotfilesConfig, McpServer, RepoConfig, ToolsConfig, UserConfig};

/// Provider when the user config names none.
const DEFAULT_PROVIDER: &str = "local-container";

/// The layer a resolved value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Layer {
    /// A command-line flag.
    Flag,
    /// The repo config (`.moor.yaml`).
    Repo,
    /// The user config.
    User,
    /// A built-in default.
    Default,
    /// The agent profile.
    Agent,
    /// Read from the forge: the account of the requester's GitHub token.
    Forge,
}

impl Layer {
    /// Lowercase name, as printed by `env`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Flag => "flag",
            Self::Repo => "repo",
            Self::User => "user",
            Self::Default => "default",
            Self::Agent => "agent",
            Self::Forge => "forge",
        }
    }
}

/// A resolved value and the layer it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sourced<T> {
    /// The value.
    pub value: T,
    /// Where it came from.
    pub source: Layer,
}

impl<T> Sourced<T> {
    /// Pairs `value` with its `source`.
    const fn new(value: T, source: Layer) -> Self {
        Self { value, source }
    }
}

/// Command-line overrides that take part in settings resolution.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Flags {
    /// `--model`.
    pub model: Option<String>,
}

/// One secret of the Box's user that the Box uses, and where it is named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredPlan {
    /// Secret name.
    pub name: String,
    /// `Repo` if the repo's `box_credentials` names it, else `User`.
    pub layer: Layer,
    /// Named by a repo or user `box_credentials` entry, not only by an MCP ref.
    pub boxed: bool,
}

/// Everything a command needs from config, resolved once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxSettings {
    /// Agent model: flag > user.
    pub model: Option<Sourced<String>>,
    /// Box provider: user > default; never the repo.
    pub provider: Sourced<String>,
    /// Box size: repo > user > default `small`.
    pub size: Sourced<BoxSize>,
    /// Sorted union of box credential names and MCP refs.
    pub credentials: Vec<CredPlan>,
    /// Repo box credential names, in repo order.
    pub box_credentials: Vec<String>,
    /// User MCP servers.
    pub mcp: BTreeMap<String, McpServer>,
    /// User dotfiles source.
    pub dotfiles: Option<DotfilesConfig>,
    /// Repo setup commands.
    pub setup: Vec<String>,
    /// Repo teardown commands.
    pub teardown: Vec<String>,
    /// Repo `env`.
    pub env: BTreeMap<String, String>,
    /// Repo compose project prefix.
    pub compose_prefix: Option<String>,
    /// User tool path overrides.
    pub tool_paths: ToolsConfig,
}

/// Builds the sorted credential plan from box credentials and MCP refs.
fn plan_credentials(uc: &UserConfig, repo: &RepoConfig) -> Vec<CredPlan> {
    let boxed: BTreeSet<&str> = repo
        .box_credentials
        .iter()
        .chain(&uc.box_credentials)
        .map(String::as_str)
        .collect();
    let mut names = boxed.clone();
    for server in uc.mcp_servers.values() {
        names.extend(server.refs());
    }
    names
        .into_iter()
        .map(|name| CredPlan {
            name: name.to_owned(),
            layer: if repo.box_credentials.iter().any(|n| n == name) {
                Layer::Repo
            } else {
                Layer::User
            },
            boxed: boxed.contains(name),
        })
        .collect()
}

impl BoxSettings {
    /// Resolves settings for one command. `repo` is `None` outside a repo.
    #[must_use]
    pub fn resolve(user: &UserConfig, repo: Option<&RepoConfig>, flags: &Flags) -> Self {
        let empty = RepoConfig::default();
        let r = repo.unwrap_or(&empty);

        let model = match (&flags.model, &user.agent.model) {
            (Some(m), _) => Some(Sourced::new(m.clone(), Layer::Flag)),
            (None, Some(m)) => Some(Sourced::new(m.clone(), Layer::User)),
            (None, None) => None,
        };
        let provider = user.box_.provider.as_ref().map_or_else(
            || Sourced::new(DEFAULT_PROVIDER.to_string(), Layer::Default),
            |p| Sourced::new(p.clone(), Layer::User),
        );
        let size = match (r.box_.size, user.box_.size) {
            (Some(s), _) => Sourced::new(s, Layer::Repo),
            (None, Some(s)) => Sourced::new(s, Layer::User),
            (None, None) => Sourced::new(BoxSize::default(), Layer::Default),
        };

        Self {
            model,
            provider,
            size,
            credentials: plan_credentials(user, r),
            box_credentials: r.box_credentials.clone(),
            mcp: user.mcp_servers.clone(),
            dotfiles: user.dotfiles.clone(),
            setup: r.setup.clone(),
            teardown: r.teardown.clone(),
            env: r.env.clone(),
            compose_prefix: r.compose.project_prefix.clone(),
            tool_paths: user.tools.clone(),
        }
    }
}

#[cfg(test)]
mod tests;
