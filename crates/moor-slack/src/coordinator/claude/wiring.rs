//! What a Claude Code turn is started with beyond its argv: the cleared environment, and the
//! `mcp.json` that names the front's tool server.

use std::fs::OpenOptions;
use std::io::{self, Write as _};
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::Path;
use std::process::Command;

use serde_json::json;

use super::HostEnv;
use crate::coordinator::ToolServer;

/// The file in a turn's workdir that names the front's tool server.
pub(super) const MCP_CONFIG: &str = "mcp.json";

/// `PATH` in a turn's cleared environment.
const PATH: &str = "/usr/local/bin:/usr/bin:/bin";

/// Writes `path`, mode 0600, naming `server` as the one stdio tool server, started by its
/// absolute path with no variables of its own.
pub(super) fn write_mcp_config(path: &Path, server: &ToolServer<'_>) -> io::Result<()> {
    let config = json!({
        "mcpServers": {
            server.name: {
                "type": "stdio",
                "command": server.program,
                "args": server.args,
                "env": {},
            }
        }
    });
    let text = serde_json::to_string_pretty(&config)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(text.as_bytes())?;
    file.write_all(b"\n")
}

/// Clears `cmd`'s environment down to `HOME`, `PATH`, `USER` and `LANG`.
pub(super) fn clear_env(cmd: &mut Command, host: &HostEnv) {
    cmd.env_clear().env("PATH", PATH).env("LANG", "C.UTF-8");
    if let Some(home) = &host.home {
        cmd.env("HOME", home);
    }
    if let Some(user) = &host.user {
        cmd.env("USER", user);
    }
}
