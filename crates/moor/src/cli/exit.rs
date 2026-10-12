//! Process exit-code table for a response envelope.

use moor_core::{ExitClass, Response};

/// Exit code when moor failed before the `run` command started.
const RUN_NOT_STARTED: u8 = 125;

/// Exit code when `wait` gave up before the Task settled.
const WAIT_TIMED_OUT: u8 = 124;

/// Maps `resp` to a process exit code.
pub(crate) fn code(resp: &Response) -> u8 {
    let is_run = resp.cmd == "run";
    match resp.class {
        ExitClass::Ok if resp.cmd == "wait" && wait_timed_out(resp) => WAIT_TIMED_OUT,
        ExitClass::Ok if is_run => run_exit(resp),
        ExitClass::Ok => 0,
        ExitClass::Failed | ExitClass::Refused | ExitClass::Usage if is_run => RUN_NOT_STARTED,
        ExitClass::Failed => 1,
        ExitClass::Usage => 2,
        ExitClass::Refused => 3,
    }
}

/// True when a `wait` value reports `timed_out`.
fn wait_timed_out(resp: &Response) -> bool {
    resp.value
        .as_ref()
        .and_then(|v| v.get("timed_out"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// Reads the command's own exit status from a `run` value, defaulting to 0.
fn run_exit(resp: &Response) -> u8 {
    resp.value
        .as_ref()
        .and_then(|v| v.get("exit"))
        .and_then(serde_json::Value::as_u64)
        .and_then(|n| u8::try_from(n).ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests;
