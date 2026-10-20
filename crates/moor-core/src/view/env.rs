//! Typed `value` payload for `env`.
//!
//! Carries `rows`, `provider` and `credentials`, plus `repo`, `agent`
//! and `github`, which reports the GitHub identity and token resolution.
//! Layers are [`crate::config::Layer`]; `agent` and `provider` come straight from
//! the resolved task settings.

use serde::{Deserialize, Serialize};

use crate::config::{BoxSettings, Layer};

/// Kind of an environment item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EnvKind {
    /// Identity.
    Identity,
    /// Credential.
    Credential,
    /// Setup command.
    Setup,
    /// MCP server.
    Mcp,
    /// Box setting.
    Box,
    /// Dotfiles.
    Dotfiles,
}

impl EnvKind {
    /// Lowercase wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Identity => "identity",
            Self::Credential => "credential",
            Self::Setup => "setup",
            Self::Mcp => "mcp",
            Self::Box => "box",
            Self::Dotfiles => "dotfiles",
        }
    }
}

/// One resolved environment item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvItem {
    /// Item kind.
    pub kind: EnvKind,
    /// Item name.
    pub name: String,
    /// Layer it came from.
    pub source: Layer,
    /// Human detail.
    pub detail: String,
    /// Whether the item resolves, for credentials and MCP.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolves: Option<bool>,
    /// Why it failed to resolve.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// How a credential reaches the agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<String>,
}

/// Agent settings.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct EnvAgent {
    /// Configured model; serializes as null when unset.
    pub model: Option<String>,
}

/// Result of a trial token mint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvMint {
    /// `ok`, `failed` or `skipped`.
    pub status: String,
    /// Failure message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Why the check was skipped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Which GitHub identity a Box agent will use. Every key serializes.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct EnvResolved {
    /// Resolved identity mode.
    pub mode: Option<String>,
    /// `yes`, `no`, `unknown` or `n/a`.
    pub app_installed: String,
    /// Where the token comes from.
    pub token_source: Option<String>,
    /// Resolution error.
    pub error: Option<String>,
}

/// GitHub identity state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvGithub {
    /// Configured identity: `you`, `bot` or `auto`.
    pub identity: String,
    /// App id; serializes as null when no App is set up.
    pub app_id: Option<String>,
    /// Login state.
    pub login: String,
    /// Trial mint outcome.
    pub mint: EnvMint,
    /// Resolved identity.
    pub resolved: EnvResolved,
}

/// Whether a row value is shown as-is or is a reference to a secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ValueKind {
    /// Plain value.
    Plain,
    /// Reference (credential).
    Ref,
}

/// One layered row; every item appears as a row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvRow {
    /// Item kind.
    pub kind: EnvKind,
    /// Item name.
    pub name: String,
    /// Layer it came from.
    pub source: Layer,
    /// `ref` for credentials, else `plain`.
    pub value_kind: ValueKind,
    /// How a credential reaches the agent; null when none.
    pub delivery: Option<String>,
    /// Human detail.
    pub note: String,
}

/// Box provider and size.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvProvider {
    /// Provider name.
    pub name: String,
    /// Layer of the provider.
    pub source: Layer,
    /// Box size.
    pub size: String,
    /// Layer of the size.
    pub size_source: Layer,
}

/// Resolution check of one credential or MCP item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvCredential {
    /// Item kind.
    pub kind: EnvKind,
    /// Item name.
    pub which: String,
    /// Whether it resolves.
    pub ok: bool,
    /// Why it failed; null when none.
    pub reason: Option<String>,
}

/// `value` of `env`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvView {
    /// Repo root.
    pub repo: String,
    /// Agent settings.
    pub agent: EnvAgent,
    /// GitHub identity state.
    pub github: EnvGithub,
    /// Layered rows in display order.
    pub rows: Vec<EnvRow>,
    /// Provider and size.
    pub provider: EnvProvider,
    /// Resolution checks.
    pub credentials: Vec<EnvCredential>,
}

impl EnvView {
    /// Build every projection from the resolved settings and the flat item list.
    #[must_use]
    pub fn new(
        repo: String,
        settings: &BoxSettings,
        github: EnvGithub,
        items: Vec<EnvItem>,
    ) -> Self {
        let agent = EnvAgent {
            model: settings.model.as_ref().map(|m| m.value.clone()),
        };
        let provider = EnvProvider {
            name: settings.provider.value.clone(),
            source: settings.provider.source,
            size: settings.size.value.as_str().to_owned(),
            size_source: settings.size.source,
        };
        let credentials = items
            .iter()
            .filter_map(|i| {
                i.resolves.map(|ok| EnvCredential {
                    kind: i.kind,
                    which: i.name.clone(),
                    ok,
                    reason: i.error.clone(),
                })
            })
            .collect();
        let rows = items
            .into_iter()
            .map(|i| EnvRow {
                kind: i.kind,
                name: i.name,
                source: i.source,
                value_kind: if i.kind == EnvKind::Credential {
                    ValueKind::Ref
                } else {
                    ValueKind::Plain
                },
                delivery: i.delivery,
                note: i.detail,
            })
            .collect();
        Self {
            repo,
            agent,
            github,
            rows,
            provider,
            credentials,
        }
    }

    /// Resolution check of `row`, matched on kind and name.
    #[must_use]
    pub fn check_of(&self, row: &EnvRow) -> Option<&EnvCredential> {
        self.credentials
            .iter()
            .find(|c| (c.kind, c.which.as_str()) == (row.kind, row.name.as_str()))
    }
}
