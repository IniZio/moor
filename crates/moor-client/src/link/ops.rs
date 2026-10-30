//! What one link stream carries.
//!
//! The runner's request is one JSON line, the agent's answer another, and for a relay the
//! server's stdio follows. A request names a hook or relay, never
//! its command, so only what the machine's owner put in its hooks dir runs as one.

use std::io::{self, Read};
use std::process::Stdio;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use moor_core::box_key::BoxKey;
use moor_core::BoxId;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::oneshot;

use super::lease;

/// Longest request or answer line.
const MAX_LINE: usize = 1 << 20;
/// Bytes of output kept, from the end.
pub const TAIL: usize = 64 * 1024;
/// How long output may keep arriving after the command's carrier exits, from children it left.
const DRAIN: Duration = Duration::from_secs(2);
/// Longest timeout a request may ask for.
pub const MAX_TIMEOUT_SECS: u64 = 3600;
/// Largest file a pull carries.
pub const MAX_PULL: u64 = 32 << 20;

/// Runs the script on fd 3 in the login shell, in its own process group, and ends that group
/// (TERM, then KILL after 5 s) once stdin reaches EOF: its carrier died or was killed. Without a
/// pty, nothing else stops the command when its call ends. POSIX sh, since macOS has no `setsid`
/// binary and dash cannot turn job control on without a tty. Its own stderr goes nowhere, so a
/// shell's job notice never reaches the output; the command keeps the real one. It sits in
/// single quotes, so it holds none.
const WRAPPER: &str = r#"exec 4<&0 </dev/null 5>&2 2>/dev/null
cmd=$(cat <&3) || exit 255
exec 3<&-
sh=${SHELL:-/bin/sh}
if command -v setsid >/dev/null 2>&1; then
  setsid "$sh" -lc "$cmd" 4<&- 2>&5 5>&- &
else
  set -m
  "$sh" -lc "$cmd" 4<&- 2>&5 5>&- &
fi
job=$!
(
  while read -r _; do :; done <&4
  kill -s TERM -- "-$job" "$job"
  sleep 5
  kill -s KILL -- "-$job" "$job"
) >/dev/null 2>&1 &
watch=$!
exec 4<&-
wait "$job"
code=$?
kill "$watch" 2>/dev/null
exit "$code"
"#;
const _: () = assert!(!has_single_quote(WRAPPER));

/// The runner's request, the first line of a stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "op", deny_unknown_fields)]
pub enum Op {
    /// Run `script` in the Box's dir through the login shell.
    Exec {
        /// The Box whose dir it runs in.
        box_id: i64,
        /// Exported before the script.
        env: Vec<(String, String)>,
        /// Shell text.
        script: String,
        /// Seconds before it is killed: 1 to [`MAX_TIMEOUT_SECS`].
        timeout_secs: u64,
    },
    /// Run the machine's own hook, if it has one, in the Box's dir.
    Hook {
        /// The Box whose dir it runs in.
        box_id: i64,
        /// Which hook.
        hook: Hook,
        /// Exported before the hook runs.
        env: Vec<(String, String)>,
        /// Seconds before it is killed: 1 to [`MAX_TIMEOUT_SECS`].
        timeout_secs: u64,
    },
    /// Start the machine's relay `name` in the Box's dir and carry its stdio.
    Relay {
        /// The Box whose dir it runs in.
        box_id: i64,
        /// `[a-z][a-z0-9-]*`, a file in the hooks dir's `relays`.
        name: String,
        /// Exported before the relay starts.
        env: Vec<(String, String)>,
    },
    /// Receive a sync into the Box's dir: the agent runs `rsync --server` on the stream.
    Sync {
        /// The Box whose dir receives it.
        box_id: i64,
        /// The remote command rsync asked its shell to run, from `rsync --server`; its
        /// destination must be the Box's dir, which the agent then names itself.
        argv: Vec<String>,
    },
    /// Send one file from the Box's dirs; its bytes follow the answer.
    Pull {
        /// The Box whose dirs it lies in.
        box_id: i64,
        /// Absolute, or relative to the Box's dir.
        path: String,
    },
    /// Listen on the machine's loopback at each of `ports` for the Box while the stream stays
    /// open, opening a stream back per accepted connection.
    Listen {
        /// The Box whose ports these are.
        box_id: i64,
        /// Names this listen in each [`crate::forward::Accepted`].
        listen: u64,
        /// Machine ports, at most [`crate::forward::MAX_PORTS`].
        ports: Vec<u16>,
    },
    /// Act on a pool lease; only a pool host serves it.
    Lease {
        /// What to do.
        verb: lease::Verb,
        /// `moor-<id>`; none for a list.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        slug: Option<String>,
        /// The OpenSSH public key an authorize lets in; none for every other verb.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key: Option<String>,
    },
    /// Carry the stream to and from the sshd of a pool lease's VM; only a pool host serves it.
    Dial {
        /// `moor-<id>`.
        slug: String,
    },
}

/// A Box lifecycle hook, a file of that name in the agent's hooks dir.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Hook {
    /// Before a Box's first call that needs its facts; its last stdout line is the facts.
    Prepare,
    /// When a prepared Box falls asleep.
    Suspend,
    /// Once the Box is no longer open.
    Release,
}

impl Hook {
    /// Its file's name.
    #[must_use]
    pub const fn file(self) -> &'static str {
        match self {
            Self::Prepare => "prepare",
            Self::Suspend => "suspend",
            Self::Release => "release",
        }
    }
}

/// The agent's answer, the line after the request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "reply", deny_unknown_fields)]
pub enum Reply {
    /// The exec or hook ended.
    Ran(Ran),
    /// The relay runs, or the dial reached the VM; its bytes follow on the stream.
    Started,
    /// What a lease request came to.
    Leased(lease::Answer),
    /// The machine has no such hook or relay.
    Missing,
    /// The pulled file's `bytes` follow on the stream.
    Sending {
        /// Its length.
        bytes: u64,
    },
    /// Every port of the listen is bound; the agent holds them until the stream closes.
    Listening,
    /// The request was not served.
    Refused {
        /// Why.
        reason: String,
    },
}

/// What a command did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ran {
    /// Its exit code; none when it was killed or ended by a signal.
    pub exit_code: Option<i32>,
    /// Whether it ran out of time and was killed.
    pub timed_out: bool,
    /// The last [`TAIL`] bytes of combined stdout and stderr.
    pub output: String,
    /// Whether earlier output was cut.
    pub truncated: bool,
}

/// Writes `value` as one JSON line.
///
/// # Errors
/// The stream is closed.
pub async fn write_line<W, T>(w: &mut W, value: &T) -> io::Result<()>
where
    W: AsyncWrite + Unpin + Send,
    T: Serialize + Sync,
{
    let mut line = serde_json::to_vec(value)?;
    line.push(b'\n');
    w.write_all(&line).await?;
    w.flush().await
}

/// Reads one JSON line, byte by byte so nothing after it is taken from the stream.
///
/// # Errors
/// The stream ends first, the line is too long, or it is not a `T`.
pub async fn read_line<R, T>(r: &mut R) -> Result<T, String>
where
    R: AsyncRead + Unpin + Send,
    T: DeserializeOwned,
{
    let mut line = Vec::new();
    loop {
        let byte = r
            .read_u8()
            .await
            .map_err(|e| format!("the stream ended: {e}"))?;
        if byte == b'\n' {
            break;
        }
        if line.len() == MAX_LINE {
            return Err("the line is too long".to_owned());
        }
        line.push(byte);
    }
    serde_json::from_slice(&line).map_err(|e| format!("not understood: {e}"))
}

/// `body` after exporting each of `env`, as one shell script.
#[must_use]
pub fn script(env: &[(String, String)], body: &str) -> String {
    let mut parts: Vec<String> = env
        .iter()
        .map(|(k, v)| format!("export {k}={}; ", quote(v)))
        .collect();
    parts.push(body.to_owned());
    parts.concat()
}

/// `body` run in Box `id`'s dir under the home, with a state dir of its own and a `TMPDIR` in
/// it, so what the Box's tools write there goes with the Box.
///
/// `body` starts its own line: after `cd ... &&`, a trailing `&` in it would background the `cd`
/// too, in a subshell that keeps ssh's output open for as long as the job runs.
#[must_use]
pub fn in_box(id: i64, body: &str) -> String {
    let key = BoxKey::new(BoxId::new(id));
    let (dir, state) = (key.machine_dir(), key.machine_state_dir());
    format!(
        "export MOOR_STATE_DIR=\"$HOME/{state}\"; export TMPDIR=\"$MOOR_STATE_DIR/tmp\"; \
         mkdir -p \"$TMPDIR\" || exit 1; mkdir -p ~/{dir} && cd ~/{dir} || exit 1\n{body}"
    )
}

/// `s` as one single-quoted word for a POSIX shell.
#[must_use]
pub fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Runs `script` under the wrapper through `carrier`, within `timeout`.
///
/// `carrier` takes the wrapper's shell line as its last argument (`ssh … target` or
/// `/bin/sh -c`). Dropping the future ends the command's process group, as its carrier's
/// stdin closes.
///
/// # Errors
/// The carrier cannot be started.
pub async fn run(mut carrier: Command, script: &str, timeout: Duration) -> io::Result<Ran> {
    let mut end = String::from("MOOR_END");
    while script.lines().any(|line| line == end) {
        end.push('_');
    }
    let line = format!("exec /bin/sh -c '{WRAPPER}' 3<<'{end}'\n{script}\n{end}\n");
    let (mut reader, writer) = io::pipe()?;
    let mut child = carrier
        .arg(line)
        .stdin(Stdio::piped())
        .stdout(writer.try_clone()?)
        .stderr(writer)
        .kill_on_drop(true)
        .spawn()?;
    drop(carrier);
    let stdin = child.stdin.take();
    let tail = Arc::new(Mutex::new(Tail::default()));
    let (done, drained) = oneshot::channel();
    let sink = Arc::clone(&tail);
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        while let Ok(n @ 1..) = reader.read(&mut buf) {
            if let Some(chunk) = buf.get(..n) {
                sink.lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(chunk);
            }
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
    drop(stdin);
    if !timed_out {
        let _ = tokio::time::timeout(DRAIN, drained).await;
    }
    let mut tail = std::mem::take(&mut *tail.lock().unwrap_or_else(PoisonError::into_inner));
    tail.trim();
    Ok(Ran {
        exit_code: status.and_then(|s| s.code()),
        timed_out,
        output: String::from_utf8_lossy(&tail.bytes).into_owned(),
        truncated: tail.cut,
    })
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

/// Whether `s` holds a `'`.
#[expect(clippy::indexing_slicing, reason = "i stays below the length")]
const fn has_single_quote(s: &str) -> bool {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\'' {
            return true;
        }
        i += 1;
    }
    false
}

#[cfg(test)]
mod tests;
