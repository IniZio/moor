//! Everything that knows Claude Code: the version gate, the argv and cleared environment of a
//! turn, the front Proxy it reaches through `HTTPS_PROXY`, the pasted token a login check reads
//! on a pipe at fd 3, the `mcp.json` that names the front's tool server, the `stream-json`
//! events of its output, and the instruction files it would load.

use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use moor_core::secret::Secret;
use serde_json::Value;

use super::process::{now, supervise, Credential, Left, Next};
use super::{Agent, Answer, Event, Loaded, Plugin, TurnSetup};
use crate::credentials::{AgentKind, CheckError, CredentialCheck};

/// Oldest Claude Code that reads a login check's token from the descriptor [`TOKEN_FD_VAR`]
/// names.
const MIN_VERSION: [u32; 3] = [2, 1, 304];

/// The variable that names a login check's token pipe; it holds the number, never the token.
const TOKEN_FD_VAR: &str = "CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR";

/// The token pipe's descriptor in the child.
const TOKEN_FD: i32 = 3;

/// Model of the token check, the cheapest one that answers.
const CHECK_MODEL: &str = "haiku";

/// Input of the token check.
const CHECK_INPUT: &str = "Reply OK.";

/// Longest the token check runs before its process group is killed.
const CHECK_LIMIT: Duration = Duration::from_secs(60);

/// Longest `--version` runs before its process group is killed.
const VERSION_LIMIT: Duration = Duration::from_secs(30);

/// What the login modal asks for.
pub(crate) const LOGIN_PROMPT: &str =
    "Paste the token that `claude setup-token` printed on your machine.";

/// What every token `claude setup-token` prints starts with.
const TOKEN_PREFIX: &str = "sk-ant-";

/// Longest token taken.
const MAX_TOKEN: usize = 512;

/// Longest reason taken from Claude's output.
const MAX_REASON: usize = 300;

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

/// What one process run left behind.
struct Ran {
    /// Standard output, up to 1 MiB.
    stdout: Vec<u8>,
    /// Standard error, up to 1 MiB.
    stderr: Vec<u8>,
    /// True when the run hit its limit and its group was killed.
    timed_out: bool,
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
        let ran = run(cmd, b"", None, VERSION_LIMIT).map_err(|e| format!("{shown}: {e}"))?;
        let out = String::from_utf8_lossy(&ran.stdout);
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
        plugins: Plugin::listed(event),
    }
}

impl CredentialCheck for Claude {
    /// A setup-token starts with `sk-ant-` and holds only letters, digits, `-` and `_`.
    fn check_format(&self, token: &Secret) -> Result<(), String> {
        let token = token.expose();
        let shaped = token.starts_with(TOKEN_PREFIX.as_bytes())
            && token.len() <= MAX_TOKEN
            && token
                .iter()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'));
        if shaped {
            Ok(())
        } else {
            Err("it is not a token that `claude setup-token` prints".to_owned())
        }
    }

    /// Runs one turn on `haiku` with `token` on the pipe and the input "Reply OK.", with
    /// `config_dir`; `Ok` when Claude answers without an error.
    fn check(&self, config_dir: &Path, token: &Secret) -> Result<(), CheckError> {
        let unavailable = |why: String| CheckError::Unavailable(redact(&why, token));
        let mut cmd = self.base(CHECK_MODEL, config_dir);
        cmd.env(TOKEN_FD_VAR, TOKEN_FD.to_string());
        let shown = self.binary.display();
        let ran = run(cmd, CHECK_INPUT.as_bytes(), Some(token), CHECK_LIMIT)
            .map_err(|e| unavailable(format!("{shown}: {e}")))?;
        if ran.timed_out {
            let limit = CHECK_LIMIT.as_secs();
            return Err(unavailable(format!(
                "Claude Code gave no answer in {limit} s"
            )));
        }
        let stderr = String::from_utf8_lossy(&ran.stderr);
        let result = result_event(&ran.stdout);
        let said = result.as_ref().map_or(&*stderr, |r| r.text.as_str());
        if said.contains("Not logged in") {
            let version = &self.version;
            let why = format!("Claude Code {version} did not read the token pipe");
            return Err(unavailable(why));
        }
        match result {
            Some(result) if !result.is_error => Ok(()),
            Some(result) => {
                let status = result.api_error_status.map(|s| format!("API error {s}"));
                let reason = first_line(&result.text).or(status);
                let reason = reason.unwrap_or_else(|| "an error with no text".to_owned());
                Err(CheckError::Refused(redact(&reason, token)))
            }
            None => {
                let last = stderr.lines().rev().find(|l| !l.trim().is_empty());
                let why = last.map_or("Claude Code printed no result", str::trim);
                Err(unavailable(why.to_owned()))
            }
        }
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

/// The last `result` event in `stdout`.
fn result_event(stdout: &[u8]) -> Option<ResultEvent> {
    let text = String::from_utf8_lossy(stdout);
    let event = text
        .lines()
        .rev()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|event| event.get("type").and_then(Value::as_str) == Some("result"))?;
    Some(parse_result(&event))
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

/// `text` cut to [`MAX_REASON`] characters, with `token` replaced wherever it appears.
fn redact(text: &str, token: &Secret) -> String {
    let token = String::from_utf8_lossy(token.expose());
    let text = if token.trim().is_empty() {
        text.to_owned()
    } else {
        text.replace(token.trim(), "[token]")
    };
    text.chars().take(MAX_REASON).collect()
}

/// Runs `cmd` with `input` on stdin and `token`, when given, on a pipe at [`TOKEN_FD`], and
/// collects its output. At `limit` the whole process group is killed.
fn run(cmd: Command, input: &[u8], token: Option<&Secret>, limit: Duration) -> io::Result<Ran> {
    let credential = token.map(|secret| Credential {
        secret,
        fd: TOKEN_FD,
    });
    let deadline = now() + limit;
    let ((stdout, timed_out), stderr) = supervise(cmd, input, credential, |lines| {
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
    Ok(Ran {
        stdout,
        stderr,
        timed_out,
    })
}

mod instructions;

mod wiring;
use wiring::{clear_env, on_path, proxied, write_mcp_config, CWD, MCP_CONFIG};

#[cfg(test)]
pub(crate) mod tests;
