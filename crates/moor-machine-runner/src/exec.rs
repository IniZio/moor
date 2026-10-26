//! `exec`: one command on the machine over ssh, in the Box's synced copy.

use std::io::{self, Read};
use std::process::Stdio;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tokio::sync::oneshot;

use crate::caller::Caller;

/// Timeout when the call names none.
pub(crate) const DEFAULT_TIMEOUT_SECS: u64 = 600;
/// Longest timeout a call may ask for.
pub(crate) const MAX_TIMEOUT_SECS: u64 = 3600;
/// Wait for held resources when the call names none.
pub(crate) const DEFAULT_HOLD_WAIT_SECS: u64 = 900;
/// Longest wait for held resources a call may ask for.
pub(crate) const MAX_HOLD_WAIT_SECS: u64 = 3600;
/// Bytes of output kept, from the end.
const TAIL: usize = 64 * 1024;
/// How long output may keep arriving after ssh exits, from children it left behind.
const DRAIN: Duration = Duration::from_secs(2);

/// Arguments of `exec`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(crate) struct ExecArgs {
    /// Shell command to run in the Box's copy on the machine.
    pub(crate) command: String,
    /// Seconds before the command is killed: default 600, at most 3600.
    pub(crate) timeout_secs: Option<u64>,
    /// Resources to hold one unit of while the command runs, such as `build`.
    #[serde(default)]
    pub(crate) hold: Vec<String>,
    /// Seconds to wait for the held resources: default 900, at most 3600.
    pub(crate) hold_wait_secs: Option<u64>,
}

/// What a command did.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub(crate) struct ExecOutput {
    /// The command's exit code; none when it was killed or ended by a signal.
    pub(crate) exit_code: Option<i32>,
    /// Whether the command ran out of time and was killed.
    pub(crate) timed_out: bool,
    /// The last 64 KiB of combined stdout and stderr.
    pub(crate) output: String,
    /// Whether earlier output was cut.
    pub(crate) truncated: bool,
}

/// The end of a stream: at most [`TAIL`] bytes, and whether more came before them.
#[derive(Default)]
struct Tail {
    /// Kept bytes; grows to twice [`TAIL`] before the front is dropped.
    bytes: Vec<u8>,
    /// Whether bytes were dropped from the front.
    cut: bool,
}

impl Tail {
    /// Appends `chunk`, dropping the front once the buffer passes twice [`TAIL`].
    fn push(&mut self, chunk: &[u8]) {
        self.bytes.extend_from_slice(chunk);
        if self.bytes.len() > 2 * TAIL {
            self.trim();
        }
    }

    /// Cuts the buffer to its last [`TAIL`] bytes.
    fn trim(&mut self) {
        if let Some(extra) = self.bytes.len().checked_sub(TAIL).filter(|n| *n > 0) {
            self.bytes.drain(..extra);
            self.cut = true;
        }
    }
}

/// Runs `command` on the caller's machine within `timeout`.
pub(crate) async fn run(
    caller: &Caller,
    command: &str,
    timeout: Duration,
) -> io::Result<ExecOutput> {
    let script = format!("cd ~/{} && {command}", caller.remote_dir());
    let remote = format!("exec \"${{SHELL:-/bin/sh}}\" -lc {}", quote(&script));
    let (mut reader, writer) = io::pipe()?;
    let mut child = Command::new("ssh")
        .args(["-o", "BatchMode=yes", "--", &caller.ssh, &remote])
        .stdin(Stdio::null())
        .stdout(writer.try_clone()?)
        .stderr(writer)
        .kill_on_drop(true)
        .spawn()?;
    let tail = Arc::new(Mutex::new(Tail::default()));
    let (done, drained) = oneshot::channel();
    let sink = Arc::clone(&tail);
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        while let Ok(n @ 1..) = reader.read(&mut buf) {
            sink.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(&buf[..n]);
        }
        let _ = done.send(());
    });
    let (status, timed_out) = if let Ok(status) = tokio::time::timeout(timeout, child.wait()).await
    {
        (Some(status?), false)
    } else {
        child.start_kill()?;
        child.wait().await?;
        (None, true)
    };
    if !timed_out {
        let _ = tokio::time::timeout(DRAIN, drained).await;
    }
    let mut tail = std::mem::take(&mut *tail.lock().unwrap_or_else(PoisonError::into_inner));
    tail.trim();
    Ok(ExecOutput {
        exit_code: status.and_then(|s| s.code()),
        timed_out,
        output: String::from_utf8_lossy(&tail.bytes).into_owned(),
        truncated: tail.cut,
    })
}

/// `s` as one single-quoted word for a POSIX shell.
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}
