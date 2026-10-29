//! `exec`: one command on the machine over ssh, in the Box's synced copy.

use std::io::{self, Read};
use std::process::Stdio;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use moor_client::link::ops;
use rmcp::ErrorData;
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tokio::sync::oneshot;

use crate::caller::Reach;
use crate::link;

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
/// Runs the script on fd 3 in the login shell, in its own process group, and ends that group
/// (TERM, then KILL after 5 s) once stdin reaches EOF: ssh died or was killed. Without a pty,
/// nothing else stops the command when its call ends. POSIX sh, since macOS has no `setsid`
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

/// `body` after exporting each of `env`, as one shell script.
pub(crate) fn script(env: &[(String, String)], body: &str) -> String {
    let mut parts: Vec<String> = env
        .iter()
        .map(|(k, v)| format!("export {k}={}; ", quote(v)))
        .collect();
    parts.push(body.to_owned());
    parts.concat()
}

/// Runs `script` at the ssh target `ssh` through the login shell, within `timeout`; the script
/// ends when this call does.
pub(crate) async fn run(ssh: &str, script: &str, timeout: Duration) -> io::Result<ExecOutput> {
    let mut end = String::from("MOOR_END");
    while script.lines().any(|line| line == end) {
        end.push('_');
    }
    let remote = format!("exec /bin/sh -c '{WRAPPER}' 3<<'{end}'\n{script}\n{end}\n");
    let (mut reader, writer) = io::pipe()?;
    let mut child = Command::new("ssh")
        .args(["-o", "BatchMode=yes", "--", ssh, &remote])
        .stdin(Stdio::piped())
        .stdout(writer.try_clone()?)
        .stderr(writer)
        .kill_on_drop(true)
        .spawn()?;
    let stdin = child.stdin.take();
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
    drop(stdin);
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
pub(crate) fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Whether `s` holds a `'`.
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

/// `secs` as a duration when it is 1 to `max`, else an error naming `name`.
pub(crate) fn bounded(name: &str, secs: u64, max: u64) -> Result<Duration, ErrorData> {
    if secs == 0 || secs > max {
        return Err(ErrorData::invalid_params(
            format!("{name} must be 1 to {max}, not {secs}"),
            None,
        ));
    }
    Ok(Duration::from_secs(secs))
}

/// Runs `command` in the Box's copy on its machine with `facts` exported, over the machine's
/// link when it is enrolled and over ssh otherwise.
pub(crate) async fn on_machine(
    agents: Option<&crate::Agents>,
    caller: &crate::caller::Caller,
    facts: &crate::hooks::Facts,
    command: &str,
    timeout: Duration,
) -> Result<ExecOutput, String> {
    let env = crate::hooks::fact_env(facts);
    let machine_id = match &caller.reach {
        Reach::Ssh { ssh } => {
            let prelude = crate::hooks::state_prelude(caller.box_id.get());
            let body = format!("{prelude}cd ~/{} && {command}", caller.remote_dir());
            return run(ssh, &script(&env, &body), timeout)
                .await
                .map_err(|e| format!("could not run ssh: {e}"));
        }
        Reach::Linked { machine_id, .. } => *machine_id,
    };
    let op = ops::Op::Exec {
        box_id: caller.box_id.get(),
        env,
        script: command.to_owned(),
        timeout_secs: link::secs(timeout),
    };
    match link::send(agents, &caller.machine, machine_id, &op).await? {
        link::Answer::Ran(ran) => Ok(ran.into()),
        _ => Err(format!("machine {} answered no run", caller.machine)),
    }
}

impl From<ops::Ran> for ExecOutput {
    fn from(ran: ops::Ran) -> Self {
        Self {
            exit_code: ran.exit_code,
            timed_out: ran.timed_out,
            output: ran.output,
            truncated: ran.truncated,
        }
    }
}
