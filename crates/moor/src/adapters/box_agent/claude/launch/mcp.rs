//! The Box's MCP servers in Claude Code's forms: the `--mcp-config` value and tool denials.

use moor_core::boxscript::quote;
use moor_core::seam::agent_profile::LaunchCtx;
use serde_json::{json, Value};

/// The Box's MCP servers in Claude Code's `mcpServers` form.
pub(super) fn config(ctx: &LaunchCtx) -> Value {
    let servers: serde_json::Map<String, Value> = ctx
        .mcp
        .iter()
        .map(|(name, s)| {
            let entry = match (&s.command, &s.url) {
                (Some(command), _) => json!({
                    "type": s.kind.as_deref().unwrap_or("stdio"),
                    "command": command,
                    "args": s.args,
                    "env": s.env,
                }),
                (None, url) => json!({
                    "type": s.kind.as_deref().unwrap_or("http"),
                    "url": url,
                    "headers": s.headers,
                }),
            };
            (name.clone(), entry)
        })
        .collect();
    json!({ "mcpServers": servers })
}

/// `PreToolUse` entries that refuse each server's tools outside its `allow_tools`; Claude Code
/// matches tool names as regular expressions and blocks a call whose hook exits 2.
pub(super) fn tool_denials(ctx: &LaunchCtx) -> Vec<Value> {
    ctx.mcp
        .iter()
        .filter_map(|(name, s)| {
            let allowed = s.allow_tools.as_ref()?;
            let why = format!("moor: the {name} MCP server allows only: {}", allowed.join(", "));
            Some(json!({
                "matcher": format!("^mcp__{name}__(?!({})$)", allowed.join("|")),
                "hooks": [{ "type": "command", "command": format!("echo {} >&2; exit 2", quote(&why)) }],
            }))
        })
        .collect()
}
