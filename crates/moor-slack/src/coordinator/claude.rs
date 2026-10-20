//! Everything that knows Claude Code: the version gate, the argv and cleared environment of a
//! turn, the token on a pipe at fd 3, and the `result` event of `stream-json` output.

use std::io::{self, Read, Write as _};
use std::os::fd::OwnedFd;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use command_fds::{CommandFdExt as _, FdMapping};
use moor_core::secret::Secret;

use crate::credentials::{CheckError, CredentialCheck};
use rustix::process::{kill_process_group, Pid, Signal};
use serde_json::Value;

/// Oldest Claude Code that reads its token from the descriptor [`TOKEN_FD_VAR`] names.
const MIN_VERSION: [u32; 3] = [2, 1, 304];

/// The variable that names the token pipe's descriptor; it holds the number, never the token.
const TOKEN_FD_VAR: &str = "CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR";

/// The token pipe's descriptor in the child.
const TOKEN_FD: i32 = 3;

/// `PATH` in a turn's cleared environment.
const PATH: &str = "/usr/local/bin:/usr/bin:/bin";

/// Model of the token check, the cheapest one that answers.
const CHECK_MODEL: &str = "haiku";

/// Input of the token check.
const CHECK_INPUT: &str = "Reply OK.";

/// Longest the token check runs before its process group is killed.
const CHECK_LIMIT: Duration = Duration::from_secs(60);

/// Longest `--version` runs before its process group is killed.
const VERSION_LIMIT: Duration = Duration::from_secs(30);

/// How long a process that closed its output may take to exit before its group is killed.
const EXIT_GRACE: Duration = Duration::from_secs(5);

/// Most bytes kept from one output stream; the rest is read and dropped.
const MAX_OUTPUT: usize = 1 << 20;

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
    /// Standard output, up to [`MAX_OUTPUT`] bytes.
    stdout: Vec<u8>,
    /// Standard error, up to [`MAX_OUTPUT`] bytes.
    stderr: Vec<u8>,
    /// True when the run hit its limit and its group was killed.
    timed_out: bool,
}

impl Claude {
    /// Runs `binary --version` with a cleared environment; keeps the binary when it is
    /// 2.1.304 or newer.
    pub(crate) fn probe(binary: &Path, host: HostEnv) -> Result<Self, String> {
        let mut cmd = Command::new(binary);
        cmd.arg("--version");
        clear_env(&mut cmd, &host);
        let shown = binary.display();
        let ran = run(cmd, b"", VERSION_LIMIT).map_err(|e| format!("{shown}: {e}"))?;
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

    /// The version the binary printed.
    pub(crate) fn version(&self) -> &str {
        &self.version
    }

    /// The command of one turn on `model`, with the cleared environment, `config_dir` as
    /// Claude Code's config dir, and the descriptor number of the token pipe.
    fn turn(&self, model: &str, config_dir: &Path) -> Command {
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
        cmd.env("CLAUDE_CONFIG_DIR", config_dir)
            .env(TOKEN_FD_VAR, TOKEN_FD.to_string());
        cmd
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

    /// Runs one turn on `haiku` with `token` on the pipe and the input "Reply OK.", in
    /// `config_dir`; `Ok` when Claude answers without an error.
    fn check(&self, config_dir: &Path, token: &Secret) -> Result<(), CheckError> {
        let unavailable = |why: String| CheckError::Unavailable(redact(&why, token));
        let mut cmd = self.turn(CHECK_MODEL, config_dir);
        cmd.current_dir(config_dir);
        let pipe = token_pipe(token).map_err(|e| unavailable(format!("token pipe: {e}")))?;
        cmd.fd_mappings(vec![FdMapping {
            parent_fd: pipe,
            child_fd: TOKEN_FD,
        }])
        .map_err(|_| unavailable("token pipe: descriptor in use".to_owned()))?;
        let shown = self.binary.display();
        let ran = run(cmd, CHECK_INPUT.as_bytes(), CHECK_LIMIT)
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

/// Clears `cmd`'s environment down to `HOME`, `PATH`, `USER` and `LANG`.
fn clear_env(cmd: &mut Command, host: &HostEnv) {
    cmd.env_clear().env("PATH", PATH).env("LANG", "C.UTF-8");
    if let Some(home) = &host.home {
        cmd.env("HOME", home);
    }
    if let Some(user) = &host.user {
        cmd.env("USER", user);
    }
}

/// A pipe that holds `token` with its write end closed; returns the read end.
fn token_pipe(token: &Secret) -> io::Result<OwnedFd> {
    let (reader, mut writer) = io::pipe()?;
    writer.write_all(token.expose())?;
    drop(writer);
    Ok(OwnedFd::from(reader))
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
    Some(ResultEvent {
        is_error: event.get("is_error").and_then(Value::as_bool) != Some(false),
        text: event
            .get("result")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        api_error_status: event.get("api_error_status").and_then(Value::as_u64),
    })
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

/// Runs `cmd` in its own process group with `input` on stdin, and collects its output. At
/// `limit` the whole group is killed. Any process the group still holds after the leader
/// exits is killed too, so nothing outlives the run.
fn run(mut cmd: Command, input: &[u8], limit: Duration) -> io::Result<Ran> {
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = cmd.spawn()?;
    let group = Pid::from_child(&child);
    let stdin = child.stdin.take();
    let input = input.to_vec();
    let writer = thread::spawn(move || {
        if let Some(mut stdin) = stdin {
            let _ = stdin.write_all(&input);
        }
    });
    let (done_tx, done) = mpsc::channel();
    let stdout = child.stdout.take();
    let out = thread::spawn(move || {
        let bytes = read_capped(stdout);
        let _ = done_tx.send(());
        bytes
    });
    let stderr = child.stderr.take();
    let err = thread::spawn(move || read_capped(stderr));
    let timed_out = matches!(done.recv_timeout(limit), Err(RecvTimeoutError::Timeout));
    if !timed_out {
        wait_up_to(&mut child, EXIT_GRACE)?;
    }
    let _ = kill_process_group(group, Signal::KILL);
    child.wait()?;
    let _ = writer.join();
    Ok(Ran {
        stdout: out.join().unwrap_or_default(),
        stderr: err.join().unwrap_or_default(),
        timed_out,
    })
}

/// Waits for `child` to exit, at most `limit`.
fn wait_up_to(child: &mut Child, limit: Duration) -> io::Result<()> {
    let step = Duration::from_millis(20);
    let mut waited = Duration::ZERO;
    while child.try_wait()?.is_none() && waited < limit {
        thread::sleep(step);
        waited += step;
    }
    Ok(())
}

/// Reads `stream` to its end and keeps the first [`MAX_OUTPUT`] bytes.
fn read_capped(stream: Option<impl Read>) -> Vec<u8> {
    let Some(mut stream) = stream else {
        return Vec::new();
    };
    let mut kept = Vec::new();
    let mut chunk = [0_u8; 8192];
    while let Ok(n) = stream.read(&mut chunk) {
        let Some(read) = chunk.get(..n).filter(|r| !r.is_empty()) else {
            break;
        };
        let room = MAX_OUTPUT.saturating_sub(kept.len());
        kept.extend(read.iter().take(room));
    }
    kept
}

#[cfg(test)]
pub(crate) mod tests;
