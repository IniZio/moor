//! What a Claude Code turn is started with beyond its argv: the binary found on `PATH` when
//! the config names none, the working directory, the cleared environment, the way to the
//! front Proxy, and the `mcp.json` that names the front's tool server.

use std::ffi::OsStr;
use std::fs::OpenOptions;
use std::io::{self, Write as _};
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::json;

use super::HostEnv;
use crate::coordinator::{ProxyAccess, ToolServer};

/// The file in a turn's workdir that names the front's tool server.
pub(super) const MCP_CONFIG: &str = "mcp.json";

/// The working directory of every turn and of the token check, so that no `CLAUDE.md` above
/// the state dir, such as `~/.claude/CLAUDE.md`, is a project file of the turn.
pub(super) const CWD: &str = "/";

/// What a turn's `CLAUDE_CODE_OAUTH_TOKEN` holds; the Proxy injects the real token in its place.
const OAUTH_PLACEHOLDER: &str = "moor-proxy";

/// `PATH` in a turn's cleared environment.
const PATH: &str = "/usr/local/bin:/usr/bin:/bin";

/// The first executable `claude` in an absolute directory of `search`.
pub(super) fn on_path(search: Option<&OsStr>) -> Result<PathBuf, String> {
    search
        .into_iter()
        .flat_map(std::env::split_paths)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join("claude"))
        .find(|path| {
            std::fs::metadata(path)
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
        .ok_or_else(|| {
            "found no claude on PATH; name its absolute path in coordinator.binary".to_owned()
        })
}

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

/// Clears `cmd`'s environment down to `HOME`, `PATH`, `USER` and `LANG`, and turns off every
/// `CLAUDE.md` Claude Code would otherwise find.
pub(super) fn clear_env(cmd: &mut Command, host: &HostEnv) {
    cmd.env_clear()
        .env("PATH", PATH)
        .env("LANG", "C.UTF-8")
        .env("CLAUDE_CODE_DISABLE_CLAUDE_MDS", "1");
    if let Some(home) = &host.home {
        cmd.env("HOME", home);
    }
    if let Some(user) = &host.user {
        cmd.env("USER", user);
    }
}

/// Sends `cmd`'s Claude traffic to the front Proxy as `proxy` says. The OAuth variable holds a
/// placeholder the Proxy replaces, the lease is the proxy password, and only the requests a
/// turn needs are made, each retried at most twice, so a Proxy that is down ends the turn soon.
pub(super) fn proxied(cmd: &mut Command, proxy: &ProxyAccess<'_>) {
    let lease = String::from_utf8_lossy(proxy.lease.expose());
    cmd.env(
        "HTTPS_PROXY",
        format!("http://lease:{lease}@127.0.0.1:{}", proxy.port),
    )
    .env("NODE_EXTRA_CA_CERTS", proxy.ca)
    .env("CLAUDE_CODE_OAUTH_TOKEN", OAUTH_PLACEHOLDER)
    .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
    .env("CLAUDE_CODE_MAX_RETRIES", "2");
}
