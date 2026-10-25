//! The Box's MCP servers in Claude Code's forms: the `--mcp-config` value.

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
