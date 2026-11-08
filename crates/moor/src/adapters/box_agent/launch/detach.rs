//! The agent runs in a detached terminal on the Box, held by herdr-core, so a dropped link leaves
//! the agent running and a client can attach to it.

use std::time::Duration;

use moor_core::boxscript::quote;
use moor_core::seam::driver::{Driver, ExecOutcome, ExecSpec};
use moor_core::seam::engine::{EngineError, SessionRef};
use moor_core::seam::proc::Deadline;

use super::holder;
use super::AGENT_PATH_LINE;

/// The Box command that replaces any agent terminal with one running the script file at
/// `script`. A Box without herdr-core refuses to start the agent: on the bare link a dropped
/// connection would end it.
pub(super) fn start(script: &str) -> String {
    format!(
        "{AGENT_PATH_LINE}; if ! command -v {} >/dev/null 2>&1; then \
         echo {} >&2; exit {NO_HOLDER_EXIT}; fi; {}",
        holder::PROGRAM,
        quote(&refusal()),
        holder::open(script),
    )
}

/// What a Box without herdr-core prints before refusing to start the agent.
pub(super) fn refusal() -> String {
    format!(
        "moor: {} is not installed on the Box; the agent needs it to outlive a dropped link",
        holder::PROGRAM
    )
}

/// The exit status of that refusal, the shell's command-not-found status.
pub(super) const NO_HOLDER_EXIT: u8 = 127;

/// The command that relays the agent's terminal through `connect`.
pub(crate) fn relay_argv(connect: &[String]) -> Vec<String> {
    let mut argv = connect.to_vec();
    argv.push(format!("{AGENT_PATH_LINE}; {}", holder::relay()));
    argv
}

/// The Box command that ends the agent's terminal; it succeeds when there is none.
fn stop_cmd() -> String {
    format!(
        "{AGENT_PATH_LINE}; ! command -v {} >/dev/null 2>&1 || {} 2>/dev/null || true",
        holder::PROGRAM,
        holder::kill()
    )
}

/// Time allowed for ending the agent's terminal on the Box.
const STOP_DEADLINE: Duration = Duration::from_secs(30);

/// Ends the agent's terminal on the Box, which hangs the agent up; a terminal already gone is
/// fine.
pub(in crate::adapters::box_agent) fn stop(
    driver: &dyn Driver,
    s: &SessionRef,
) -> Result<(), EngineError> {
    let spec = ExecSpec {
        argv: vec!["sh".to_owned(), "-c".to_owned(), stop_cmd()],
        env: Vec::new(),
        cwd: None,
        output: None,
    };
    let failed = |message: String| EngineError::Failed {
        step: "stop",
        message,
    };
    match driver.exec(&s.handle, &spec, Deadline(STOP_DEADLINE)) {
        Ok(ExecOutcome::Exited { code: 0 }) => Ok(()),
        Ok(ExecOutcome::Exited { code }) => Err(failed(format!("exited {code}"))),
        Ok(ExecOutcome::Interrupted) => Err(failed("interrupted".to_owned())),
        Err(e) => Err(failed(e.to_string())),
    }
}

#[cfg(test)]
#[path = "tests/detach.rs"]
mod tests;
