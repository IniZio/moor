//! Typed `value` payload for `env`.
//!
//! Carries `rows`, `provider` and `credentials`, plus `repo` and `agent`.
//! Layers are [`crate::config::Layer`]; `agent` and `provider` come straight from
//! the resolved box settings.

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
    /// Cache volume.
    Cache,
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
            Self::Cache => "cache",
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
    /// Provider name; `None` when the user config names none.
    pub name: Option<String>,
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
    pub fn new(repo: String, settings: &BoxSettings, items: Vec<EnvItem>) -> Self {
        let agent = EnvAgent {
            model: settings.model.as_ref().map(|m| m.value.clone()),
        };
        let provider = EnvProvider {
            name: settings.provider.as_ref().map(|p| p.value.clone()),
            source: settings
                .provider
                .as_ref()
                .map_or(Layer::Default, |p| p.source),
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
