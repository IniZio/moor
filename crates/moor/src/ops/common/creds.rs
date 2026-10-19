//! Credential and MCP rows shared by `env` and the `start` preflight.
//!
//! Each distinct credential source is checked at most once per call, so a source
//! shared by several rows never re-runs its `cmd`.

use std::cell::RefCell;
use std::path::Path;

use moor_core::config::{check, BoxSettings, CredPlan, CredentialSource, Layer};
use moor_core::view::env::{EnvItem, EnvKind};

use crate::adapters::creds::describe;
use crate::cx::Cx;

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

/// Every credential row, then user MCP rows, then repo MCP rows.
pub(crate) fn checked(cx: &Cx, s: &BoxSettings, worktree: &Path) -> Checked {
    let memo = Memo::new(cx);
    let credentials = s
        .credentials
        .iter()
        .map(|c| credential_item(&memo, c))
        .collect();
    let mut mcp = user_mcp_items(&memo, s);
    mcp.extend(repo_mcp_items(worktree));
    Checked { credentials, mcp }
}

/// Credential and MCP items that do not resolve, limited to what this Task uses
/// (credentials with `used == true`, every user MCP server, every repo `.mcp.json` server).
/// Empty means the preflight passes.
pub(crate) fn unresolved(cx: &Cx, s: &BoxSettings, worktree: &Path) -> Vec<EnvItem> {
    let memo = Memo::new(cx);
    let used: Vec<EnvItem> = s
        .used_credentials()
        .map(|c| credential_item(&memo, c))
        .chain(user_mcp_items(&memo, s))
        .chain(repo_mcp_items(worktree))
        .collect();
    used.into_iter()
        .filter(|i| i.resolves == Some(false))
        .collect()
}

/// Check results by source, so each distinct source is checked once.
struct Memo<'a> {
    /// Context whose credential adapter does the check.
    cx: &'a Cx,
    /// Sources already checked, with their result.
    seen: RefCell<Vec<(&'a CredentialSource, Result<(), String>)>>,
}

impl<'a> Memo<'a> {
    /// An empty memo.
    const fn new(cx: &'a Cx) -> Self {
        Self {
            cx,
            seen: RefCell::new(Vec::new()),
        }
    }

    /// The check result for `src`, running the check on first sight only.
    fn check(&self, src: &'a CredentialSource) -> Result<(), String> {
        if let Some((_, r)) = self.seen.borrow().iter().find(|(s, _)| *s == src) {
            return r.clone();
        }
        let r = self.cx.creds().check(src);
        self.seen.borrow_mut().push((src, r.clone()));
        r
    }
}

/// How a credential reaches the agent; the agent always runs in the Box.
fn delivery_label(c: &CredPlan) -> &'static str {
    if check::proxied_claude_var(&c.name) {
        "proxy"
    } else if c.boxed {
        "stdin/file"
    } else {
        "host only"
    }
}

/// One credential row.
fn credential_item<'a>(memo: &Memo<'a>, c: &'a CredPlan) -> EnvItem {
    let (detail, error) = c.source.as_ref().map_or_else(
        || {
            (
                "no source in user layer".to_owned(),
                Some(format!(
                    "no source for {0}; add credentials.{0} in ~/.config/moor/config.yaml",
                    c.name
                )),
            )
        },
        |src| (describe(src), memo.check(src).err()),
    );
    EnvItem {
        source: c.layer,
        resolves: Some(error.is_none()),
        error,
        delivery: Some(delivery_label(c).to_owned()),
        ..plain(EnvKind::Credential, &c.name, &detail)
    }
}

/// User MCP servers; each resolves iff every `${REF}` names a credential that checks ok.
fn user_mcp_items<'a>(memo: &Memo<'a>, s: &'a BoxSettings) -> Vec<EnvItem> {
    s.mcp
        .iter()
        .map(|(name, srv)| {
            let detail = match (&srv.command, &srv.url) {
                (Some(c), _) => c.split_whitespace().next().unwrap_or("").to_owned(),
                (None, Some(u)) => u.clone(),
                (None, None) => String::new(),
            };
            let error = srv.refs().into_iter().find_map(|r| {
                let src = s
                    .credentials
                    .iter()
                    .find(|c| c.name == r)
                    .and_then(|c| c.source.as_ref());
                match src.map(|src| memo.check(src)) {
                    Some(Ok(())) => None,
                    Some(Err(e)) => Some(format!("credential ${{{r}}}: {e}")),
                    None => Some(format!("credential ${{{r}}}: no source")),
                }
            });
            EnvItem {
                source: Layer::User,
                resolves: Some(error.is_none()),
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
