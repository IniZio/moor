//! Credential and MCP rows shared by `env` and the `start` preflight.
//!
//! Each credential is a secret of the Box's user. A row resolves when that user holds the
//! secret. `env` acts for no user, so its rows leave resolution open.

use std::collections::BTreeSet;
use std::path::Path;

use moor_core::config::{BoxSettings, CredPlan, Layer};
use moor_core::user::SecretName;
use moor_core::view::env::{EnvItem, EnvKind};

/// Credential rows and MCP rows, each in display order.
pub(crate) struct Checked {
    /// One row per credential.
    pub credentials: Vec<EnvItem>,
    /// User MCP rows, then repo `.mcp.json` rows.
    pub mcp: Vec<EnvItem>,
}

/// An item with no resolution state.
pub(crate) fn plain(kind: EnvKind, name: &str, detail: &str) -> EnvItem {
    EnvItem {
        kind,
        name: name.to_owned(),
        source: Layer::Repo,
        detail: detail.to_owned(),
        resolves: None,
        error: None,
        delivery: None,
    }
}

/// Every credential row, then user MCP rows, then repo MCP rows, for no user in particular.
pub(crate) fn checked(s: &BoxSettings, worktree: &Path) -> Checked {
    let credentials = s
        .credentials
        .iter()
        .map(|c| credential_item(c, None))
        .collect();
    let mut mcp = user_mcp_items(s, None);
    mcp.extend(repo_mcp_items(worktree));
    Checked { credentials, mcp }
}

/// Credential and MCP items that do not resolve for a user holding the secrets `have`.
/// Empty means the preflight passes.
pub(crate) fn unresolved(
    s: &BoxSettings,
    worktree: &Path,
    have: &BTreeSet<SecretName>,
) -> Vec<EnvItem> {
    s.credentials
        .iter()
        .map(|c| credential_item(c, Some(have)))
        .chain(user_mcp_items(s, Some(have)))
        .chain(repo_mcp_items(worktree))
        .filter(|i| i.resolves == Some(false))
        .collect()
}

/// Why secret `name` does not resolve for a user holding `have`; `None` when it does.
fn missing(name: &str, have: &BTreeSet<SecretName>) -> Option<String> {
    let held = SecretName::parse(name).is_ok_and(|n| have.contains(&n));
    (!held).then(|| format!("log in first: moor user secret set {name}"))
}

/// What moor does with a credential: it puts none into the Box, and checks a `box_credentials`
/// name only.
const fn delivery_label(c: &CredPlan) -> &'static str {
    if c.boxed {
        "name checked only"
    } else {
        "host only"
    }
}

/// One credential row; resolution is checked only against `have`.
fn credential_item(c: &CredPlan, have: Option<&BTreeSet<SecretName>>) -> EnvItem {
    let error = have.and_then(|h| missing(&c.name, h));
    EnvItem {
        source: c.layer,
        resolves: have.map(|_| error.is_none()),
        error,
        delivery: Some(delivery_label(c).to_owned()),
        ..plain(EnvKind::Credential, &c.name, "secret of the Box's user")
    }
}

/// User MCP servers; each resolves iff `have` holds every secret its `${REF}`s name.
fn user_mcp_items(s: &BoxSettings, have: Option<&BTreeSet<SecretName>>) -> Vec<EnvItem> {
    s.mcp
        .iter()
        .map(|(name, srv)| {
            let detail = match (&srv.command, &srv.url) {
                (Some(c), _) => c.split_whitespace().next().unwrap_or("").to_owned(),
                (None, Some(u)) => u.clone(),
                (None, None) => String::new(),
            };
            let error = have.and_then(|h| {
                srv.refs()
                    .into_iter()
                    .find_map(|r| missing(r, h).map(|e| format!("credential ${{{r}}}: {e}")))
            });
            EnvItem {
                source: Layer::User,
                resolves: have.map(|_| error.is_none()),
                error,
                ..plain(EnvKind::Mcp, name, &detail)
            }
        })
        .collect()
}

/// Servers approved or refused in the repo's Claude settings.
#[derive(Default)]
struct McpApproval {
    /// `enabledMcpjsonServers`.
    enabled: Vec<String>,
    /// `disabledMcpjsonServers`.
    disabled: Vec<String>,
    /// `enableAllProjectMcpServers`.
    all: bool,
}

/// Read approvals from `.claude/settings{,.local}.json`; moor never writes them.
fn mcp_approval(repo: &Path) -> McpApproval {
    let mut a = McpApproval::default();
    for f in ["settings.json", "settings.local.json"] {
        let Ok(text) = std::fs::read_to_string(repo.join(".claude").join(f)) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        let names = |key: &str| -> Vec<String> {
            v.get(key)
                .and_then(serde_json::Value::as_array)
                .map(|l| {
                    l.iter()
                        .filter_map(|s| s.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default()
        };
        a.enabled.extend(names("enabledMcpjsonServers"));
        a.disabled.extend(names("disabledMcpjsonServers"));
        a.all |= v
            .get("enableAllProjectMcpServers")
            .and_then(serde_json::Value::as_bool)
            == Some(true);
    }
    a
}

/// Repo `.mcp.json` servers; each resolves iff approved.
fn repo_mcp_items(repo: &Path) -> Vec<EnvItem> {
    let item = |name: &str, resolves: bool, error: Option<String>| EnvItem {
        resolves: Some(resolves),
        error,
        ..plain(EnvKind::Mcp, name, ".mcp.json")
    };
    let Ok(text) = std::fs::read_to_string(repo.join(".mcp.json")) else {
        return Vec::new();
    };
    let v = match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(v) => v,
        Err(e) => return vec![item(".mcp.json", false, Some(format!("invalid JSON: {e}")))],
    };
    let Some(servers) = v.get("mcpServers").and_then(serde_json::Value::as_object) else {
        return Vec::new();
    };
    let ap = mcp_approval(repo);
    servers
        .keys()
        .map(|name| {
            if ap.disabled.contains(name) {
                item(name, false, Some("disabled in .claude settings".to_owned()))
            } else if ap.all || ap.enabled.contains(name) {
                item(name, true, None)
            } else {
                item(
                    name,
                    false,
                    Some("needs approval: a dispatched agent blocks on Claude's project MCP prompt; add it to enabledMcpjsonServers in .claude/settings.local.json".to_owned()),
                )
            }
        })
        .collect()
}

#[cfg(test)]
mod tests;
