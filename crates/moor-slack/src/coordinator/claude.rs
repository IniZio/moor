//! Everything that knows Claude Code: the version gate, the argv and cleared environment of a
//! turn, the front Proxy it reaches through `HTTPS_PROXY`, the `mcp.json` that names the front's tool server, the `stream-json`
//! events of its output, and the instruction files it would load.

use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde_json::Value;

use super::process::{now, supervise, Left, Next};
use super::{Agent, Answer, Event, Loaded, TurnSetup};
use crate::credentials::AgentKind;

/// Oldest Claude Code the front runs.
const MIN_VERSION: [u32; 3] = [2, 1, 304];

/// Longest `--version` runs before its process group is killed.
const VERSION_LIMIT: Duration = Duration::from_secs(30);

/// The host values a cleared environment keeps.
#[derive(Debug, Clone, Default)]
pub(crate) struct HostEnv {
    /// `HOME` of the daemon.
    pub(crate) home: Option<PathBuf>,
    /// `USER` of the daemon.
    pub(crate) user: Option<String>,
}

/// A Claude Code binary that passed the version gate.
#[derive(Debug)]
pub(crate) struct Claude {
    /// Absolute path of the binary.
    binary: PathBuf,
    /// The version it printed, such as `2.1.304`.
    version: String,
    /// What its cleared environment keeps.
    host: HostEnv,
}

impl Claude {
    /// Probes `binary`, or without one the first `claude` in `search`, the daemon's `PATH`.
    pub(crate) fn locate(
        binary: Option<&Path>,
        search: Option<&OsStr>,
        host: HostEnv,
    ) -> Result<Self, String> {
        match binary {
            Some(binary) => Self::probe(binary, host),
            None => Self::probe(&on_path(search)?, host),
        }
    }

    /// Runs `binary --version` with a cleared environment; keeps the binary when it is
    /// 2.1.304 or newer.
    pub(crate) fn probe(binary: &Path, host: HostEnv) -> Result<Self, String> {
        let mut cmd = Command::new(binary);
        cmd.arg("--version");
        clear_env(&mut cmd, &host);
        let shown = binary.display();
        let ran = run(cmd, b"", VERSION_LIMIT).map_err(|e| format!("{shown}: {e}"))?;
        let out = String::from_utf8_lossy(&ran);
        let version = out.split_whitespace().next().unwrap_or_default().to_owned();
        match parse_version(&version) {
            Some(found) if found >= MIN_VERSION => Ok(Self {
                binary: binary.to_owned(),
                version,
                host,
            }),
            Some(_) => Err(format!("Claude Code {version} is older than 2.1.304")),
            None => Err(format!("{shown} --version printed no version")),
        }
    }

    /// The command of one run on `model` in [`CWD`], with the cleared environment and
    /// `config_dir` as Claude Code's config dir.
    fn base(&self, model: &str, config_dir: &Path) -> Command {
        let mut cmd = Command::new(&self.binary);
        cmd.args([
            "-p",
            "--no-session-persistence",
            "--model",
            model,
            "--output-format",
            "stream-json",
            "--verbose",
            "--tools",
            "",
            "--setting-sources",
            "",
            "--disable-slash-commands",
            "--strict-mcp-config",
            "--permission-prompts",
            "none",
        ]);
        clear_env(&mut cmd, &self.host);
        cmd.env("CLAUDE_CONFIG_DIR", config_dir).current_dir(CWD);
        cmd
    }
}

impl Agent for Claude {
    fn kind(&self) -> AgentKind {
        AgentKind::Claude
    }

    fn program(&self) -> &'static str {
        "Claude Code"
    }

    fn version(&self) -> &str {
        &self.version
    }

    fn turn_command(&self, setup: &TurnSetup<'_>) -> Result<Command, String> {
        let config = setup.workdir.join(MCP_CONFIG);
        write_mcp_config(&config, setup.tools)
            .map_err(|e| format!("write {}: {e}", config.display()))?;
        let mut cmd = self.base(setup.model, setup.config_dir);
        proxied(&mut cmd, setup.proxy);
        cmd.args(["--system-prompt", setup.brief])
            .arg("--mcp-config")
            .arg(&config)
            .args(["--allowedTools", &format!("mcp__{}", setup.tools.name)]);
        Ok(cmd)
    }

    fn tool_name(&self, server: &str, tool: &str) -> String {
        format!("mcp__{server}__{tool}")
    }

    fn event(&self, line: &str) -> Event {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            return Event::Other;
        };
        let field = |key: &str| event.get(key).and_then(Value::as_str);
        match (field("type"), field("subtype")) {
            (Some("system"), Some("init")) => Event::Loaded(loaded(&event)),
            (Some("result"), _) => Event::Answer(self.answer(&event)),
            _ => Event::Other,
        }
    }

    fn unsafe_cwd(&self) -> Option<String> {
        instructions::loaded(
            Path::new(CWD),
            Path::new(instructions::MANAGED),
            Path::new(instructions::MANAGED_SETTINGS),
        )
    }
}

impl Claude {
    /// What a `result` event says about the turn.
    fn answer(&self, event: &Value) -> Answer {
        let result = parse_result(event);
        if !result.is_error {
            let cost_usd = event.get("total_cost_usd").and_then(Value::as_f64);
            return Answer::Reply {
                text: result.text,
                cost_usd,
            };
        }
        if result.api_error_status == Some(401) {
            return Answer::Refused;
        }
        if result.text.contains("Not logged in") {
            let version = &self.version;
            return Answer::Unavailable(format!(
                "Claude Code {version} did not take the Proxy's placeholder login"
            ));
        }
        let status = result.api_error_status.map(|s| format!("API error {s}"));
        let line = first_line(&result.text).or(status);
        Answer::Failed(line.unwrap_or_else(|| "an error with no text".to_owned()))
    }
}

/// What a `system/init` event lists. A server or plugin is an object with a `name`.
fn loaded(event: &Value) -> Loaded {
    let names = |key: &str| -> Vec<String> {
        let items = event.get(key).and_then(Value::as_array);
        items
            .into_iter()
            .flatten()
            .filter_map(|item| {
                item.as_str()
                    .or_else(|| item.get("name").and_then(Value::as_str))
                    .map(str::to_owned)
            })
            .collect()
    };
    Loaded {
        tools: names("tools"),
        servers: names("mcp_servers"),
        skills: names("skills"),
        commands: names("slash_commands"),
        plugins: names("plugins"),
    }
}

/// `major.minor.patch`, when `text` is one.
fn parse_version(text: &str) -> Option<[u32; 3]> {
    let mut parts = text.split('.').map(str::parse::<u32>);
    let version = [
        parts.next()?.ok()?,
        parts.next()?.ok()?,
        parts.next()?.ok()?,
    ];
    parts.next().is_none().then_some(version)
}

/// The fields of a `stream-json` `result` event the front reads.
struct ResultEvent {
    /// True when the turn ended in an error.
    is_error: bool,
    /// The reply, or the error text.
    text: String,
    /// The HTTP status of an API error, such as 401.
    api_error_status: Option<u64>,
}

/// The fields of one `result` event.
fn parse_result(event: &Value) -> ResultEvent {
    ResultEvent {
        is_error: event.get("is_error").and_then(Value::as_bool) != Some(false),
        text: event
            .get("result")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        api_error_status: event.get("api_error_status").and_then(Value::as_u64),
    }
}

/// The first non-empty line of `text`, trimmed.
fn first_line(text: &str) -> Option<String> {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    Some(line.to_owned())
}

/// Runs `cmd` with `input` on stdin and collects its output. At `limit` the whole process
/// group is killed.
fn run(cmd: Command, input: &[u8], limit: Duration) -> io::Result<Vec<u8>> {
    let deadline = now() + limit;
    let ((stdout, _), _) = supervise(cmd, input, |lines| {
        let mut out = Vec::new();
        loop {
            match lines.next(deadline) {
                Next::Line(line) => {
                    out.extend(line.bytes());
                    out.push(b'\n');
                }
                Next::End => return ((out, false), Left::Ended),
                Next::Late => return ((out, true), Left::Stopped),
            }
        }
    })?;
    Ok(stdout)
}

mod instructions;

pub(crate) mod wiring;
use wiring::{clear_env, on_path, proxied, write_mcp_config, CWD, MCP_CONFIG};

#[cfg(test)]
pub(crate) mod tests;
