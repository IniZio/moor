//! Sends a steer to the agent's terminal through herdr-core on the Box.

use moor_core::boxscript::quote;
use moor_core::seam::agent_adapter::{AgentAdapter, Key};
use moor_core::seam::driver::{BoxStep, Driver, ExecOutcome, StepDone};
use moor_core::seam::engine::{EngineError, SessionRef, Steer, SteerOutcome};
use moor_core::seam::proc::Deadline;

use super::launch::holder::{PROGRAM, SESSION};
use super::launch::AGENT_PATH_LINE;

/// herdr-core's exit status when the terminal is not there.
const NOT_FOUND: i32 = 3;
/// herdr-core's exit status when the terminal's program has ended.
const FINISHED: i32 = 11;

/// Sends `steer`: text is pasted and submitted, a menu choice and the interrupt are key presses.
/// Text goes in even while the agent asks for something, as a person at the terminal could type.
pub(super) fn steer(
    driver: &dyn Driver,
    profile: &dyn AgentAdapter,
    s: &SessionRef,
    steer: &Steer,
    deadline: Deadline,
) -> Result<SteerOutcome, EngineError> {
    let (step, line) = match steer {
        Steer::Text(text) => (
            "type",
            format!("{PROGRAM} send --force --json {SESSION} {}", quote(text)),
        ),
        Steer::Menu(n) => {
            let keys: Vec<String> = profile.menu_keys(*n).into_iter().map(key_name).collect();
            (
                "menu",
                format!("{PROGRAM} keys {SESSION} {}", keys.join(" ")),
            )
        }
        Steer::Interrupt => (
            "interrupt",
            format!(
                "{PROGRAM} keys {SESSION} {}",
                key_name(profile.interrupt_key())
            ),
        ),
    };
    let steps = [BoxStep::Run(format!("{AGENT_PATH_LINE}\n{line}"))];
    let done = driver.batch(&s.handle, &[], &steps, deadline).pop();
    let stdout = match done {
        Some(Ok(StepDone::Ran {
            outcome: ExecOutcome::Exited { code: 0 },
            stdout,
        })) => stdout,
        Some(Ok(StepDone::Ran {
            outcome: ExecOutcome::Exited {
                code: NOT_FOUND | FINISHED,
            },
            ..
        })) => return Err(EngineError::AgentGone),
        Some(Ok(StepDone::Ran { outcome, .. })) => {
            return Err(failed(step, format!("herdr-core {outcome:?}")))
        }
        Some(Ok(other)) => return Err(failed(step, format!("{other:?}"))),
        Some(Err(e)) => return Err(EngineError::Box(e)),
        None => return Err(failed(step, "no answer".to_owned())),
    };
    let started = matches!(steer, Steer::Text(_))
        && serde_json::from_str::<serde_json::Value>(stdout.trim())
            .ok()
            .and_then(|v| v.get("started").and_then(serde_json::Value::as_bool))
            == Some(true);
    Ok(SteerOutcome {
        submit_retried: false,
        submit_checked: started,
    })
}

/// The key as herdr-core names it.
fn key_name(key: Key) -> String {
    match key {
        Key::Char(c) => c.to_string(),
        Key::Escape => "esc".to_owned(),
    }
}

/// A send that failed at `step`.
const fn failed(step: &'static str, message: String) -> EngineError {
    EngineError::Failed { step, message }
}

#[cfg(test)]
mod tests;
