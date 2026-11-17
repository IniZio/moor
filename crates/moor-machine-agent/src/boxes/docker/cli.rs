//! The stock `docker` command line, run as a child process.

use std::ffi::OsStr;
use std::process::{Output, Stdio};

use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::boxes::provider::ProvideError;

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
        let stdout = String::from_utf8_lossy(&out.stdout);
        if out.status.success() {
            return Ok(Some(if stdout.trim() == "true" {
                State::Running
            } else {
                State::Stopped
            }));
        }
        if String::from_utf8_lossy(&out.stderr)
            .to_lowercase()
            .contains("no such")
        {
            Ok(None)
        } else {
            Err(failed(&out))
        }
    }
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
