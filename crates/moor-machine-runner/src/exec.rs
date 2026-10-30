//! `exec`: one command on the machine over its link, in the Box's synced copy.

use std::time::Duration;

use moor_client::link::ops;
use rmcp::ErrorData;
use serde::{Deserialize, Serialize};

use crate::link;

/// Timeout when the call names none.
pub(crate) const DEFAULT_TIMEOUT_SECS: u64 = 600;
/// Longest timeout a call may ask for.
pub(crate) const MAX_TIMEOUT_SECS: u64 = 3600;
/// Wait for held resources when the call names none.
pub(crate) const DEFAULT_HOLD_WAIT_SECS: u64 = 900;
/// Longest wait for held resources a call may ask for.
pub(crate) const MAX_HOLD_WAIT_SECS: u64 = 3600;
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
/// link.
pub(crate) async fn on_machine(
    agents: Option<&crate::Agents>,
    caller: &crate::caller::Caller,
    facts: &crate::hooks::Facts,
    command: &str,
    timeout: Duration,
) -> Result<ExecOutput, String> {
    let env = crate::hooks::fact_env(facts);
    let machine_id = caller.reach.machine_id;
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
