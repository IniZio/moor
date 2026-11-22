//! The stock `docker` command line, run as a child process.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::process::{Output, Stdio};

use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use super::names::Names;
use crate::boxes::provider::{Ended, PlaceState, ProvideError};
use crate::boxes::Slug;

/// Whether a container is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum State {
    /// It runs.
    Running,
    /// It exists and is stopped.
    Stopped,
}

/// The `docker` program.
#[derive(Debug, Clone)]
pub(super) struct Cli;

impl Cli {
    /// A `docker` command with `args`; the caller adds stdio and runs it.
    pub(super) fn command(args: impl IntoIterator<Item = impl AsRef<OsStr>>) -> Command {
        let mut cmd = Command::new("docker");
        cmd.args(args).kill_on_drop(true);
        cmd
    }

    /// Runs `docker args` to its end, whatever its exit.
    pub(super) async fn output(
        args: impl IntoIterator<Item = impl AsRef<OsStr>>,
    ) -> Result<Output, ProvideError> {
        Self::command(args)
            .stdin(Stdio::null())
            .output()
            .await
            .map_err(|e| ProvideError::Failed(format!("cannot run docker: {e}")))
    }

    /// Runs `docker args` and returns its stdout; a failed exit is an error saying why.
    pub(super) async fn ok(
        args: impl IntoIterator<Item = impl AsRef<OsStr>>,
    ) -> Result<String, ProvideError> {
        let out = Self::output(args).await?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
        } else {
            Err(failed(&out))
        }
    }

    /// Whether `docker args` exits 0.
    pub(super) async fn passes(args: impl IntoIterator<Item = impl AsRef<OsStr>>) -> bool {
        Self::output(args)
            .await
            .is_ok_and(|out| out.status.success())
    }

    /// Runs `docker args` with `input` on its stdin.
    pub(super) async fn feed(
        args: impl IntoIterator<Item = impl AsRef<OsStr>>,
        input: &str,
    ) -> Result<(), ProvideError> {
        let gone = |e: std::io::Error| ProvideError::Failed(format!("cannot run docker: {e}"));
        let mut child = Self::command(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(gone)?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(input.as_bytes()).await.map_err(gone)?;
        }
        let out = child.wait_with_output().await.map_err(gone)?;
        if out.status.success() {
            Ok(())
        } else {
            Err(failed(&out))
        }
    }

    /// How container `name` stands; none when there is no such container.
    pub(super) async fn state(name: &str) -> Result<Option<State>, ProvideError> {
        let out = Self::output(["inspect", "-f", "{{.State.Running}}", name]).await?;
        read_state(
            out.status.success(),
            &String::from_utf8_lossy(&out.stdout),
            &String::from_utf8_lossy(&out.stderr),
            std::env::var("DOCKER_HOST").ok().as_deref(),
        )
    }
}

/// A container's state from what `docker inspect` said; none when it has no such container. A dead
/// daemon is an error, though its socket path reads "no such file". `host` is `DOCKER_HOST`.
pub(super) fn read_state(
    succeeded: bool,
    stdout: &str,
    stderr: &str,
    host: Option<&str>,
) -> Result<Option<State>, ProvideError> {
    if succeeded {
        return Ok(Some(if stdout.trim() == "true" {
            State::Running
        } else {
            State::Stopped
        }));
    }
    if let Some(why) = unanswered(stderr, host) {
        return Err(ProvideError::Failed(why));
    }
    let said = stderr.to_lowercase();
    if said.contains("no such container") || said.contains("no such object") {
        return Ok(None);
    }
    Err(ProvideError::Failed(if stderr.trim().is_empty() {
        "docker inspect failed".to_owned()
    } else {
        format!("docker: {}", stderr.trim())
    }))
}

/// Why the daemon does not answer, when `stderr` of a `docker` run says it did not; none for any
/// other failure. `host` is `DOCKER_HOST`.
fn unanswered(stderr: &str, host: Option<&str>) -> Option<String> {
    let said = stderr.to_lowercase();
    let down = [
        "cannot connect to the docker daemon",
        "failed to connect to the docker api",
        "is the docker daemon running",
        "error during connect",
    ]
    .iter()
    .any(|m| said.contains(m));
    let at = address_in(stderr).or(host).unwrap_or("the default address");
    down.then(|| {
        format!(
            "the Docker daemon at {at} is not answering: {}",
            stderr.trim()
        )
    })
}

/// What a failed `docker ps` of every Box means. A daemon that does not answer took every Box's
/// places with it, since their network namespace was its own.
pub(super) fn listing_failed(stderr: &str, host: Option<&str>) -> ProvideError {
    unanswered(stderr, host).map_or_else(
        || ProvideError::Failed(format!("docker: {}", stderr.trim())),
        ProvideError::Lost,
    )
}

/// The daemon address Docker's own error names, such as the `unix:///…` after "daemon at".
fn address_in(stderr: &str) -> Option<&str> {
    let (_, after) = stderr.split_once(" at ")?;
    let word = after.split_whitespace().next()?;
    let addr = word.trim_end_matches(['.', ';', ',', ':']);
    addr.contains("://").then_some(addr)
}

/// A failed `docker` run as an error.
pub(super) fn failed(out: &Output) -> ProvideError {
    let said = String::from_utf8_lossy(&out.stderr);
    let said = said.trim();
    ProvideError::Failed(if said.is_empty() {
        format!("docker exited {}", out.status)
    } else {
        format!("docker: {said}")
    })
}

/// How each place of `listed` that is down ended, by container name, from one `docker
/// inspect`; a place it cannot read is left out.
pub(super) async fn ended(
    listed: &BTreeMap<Slug, (PlaceState, PlaceState)>,
) -> BTreeMap<String, Ended> {
    let down: Vec<String> = listed
        .iter()
        .flat_map(|(slug, (agent, work))| {
            let names = Names::of(slug);
            [(names.agent, *agent), (names.work, *work)]
        })
        .filter(|(_, place)| *place == PlaceState::Down)
        .map(|(name, _)| name)
        .collect();
    if down.is_empty() {
        return BTreeMap::new();
    }
    let format = "{{.Name}} {{.State.ExitCode}} {{.State.OOMKilled}}";
    let args = ["inspect", "-f", format].into_iter().map(str::to_owned);
    Cli::output(args.chain(down))
        .await
        .map(|out| parse_ended(&String::from_utf8_lossy(&out.stdout)))
        .unwrap_or_default()
}

/// How each container ended, by name, from `docker inspect` lines of `/name code oom-killed`.
/// A line that does not read so is left out.
pub(super) fn parse_ended(inspected: &str) -> BTreeMap<String, Ended> {
    inspected
        .lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let name = words.next()?.trim_start_matches('/');
            let code = words.next()?.parse().ok()?;
            let oom = words.next()?.parse().ok()?;
            Some((name.to_owned(), Ended { code, oom }))
        })
        .collect()
}

/// The places of each Box that `docker ps` lists as `slug place state` lines: how the agent place
/// and the work place stand. A line that is not a Box's is left out.
pub(super) fn parse_ps(listed: &str) -> BTreeMap<Slug, (PlaceState, PlaceState)> {
    let mut boxes: BTreeMap<Slug, (PlaceState, PlaceState)> = BTreeMap::new();
    for line in listed.lines() {
        let mut words = line.split_whitespace();
        let (Some(slug), Some(place), Some(state)) = (
            words.next().and_then(Slug::parse),
            words.next(),
            words.next(),
        ) else {
            continue;
        };
        let up = if state == "running" {
            PlaceState::Up
        } else {
            PlaceState::Down
        };
        let places = boxes
            .entry(slug)
            .or_insert((PlaceState::Down, PlaceState::Down));
        match place {
            "agent" => places.0 = up,
            "work" => places.1 = up,
            _ => {}
        }
    }
    boxes
}
