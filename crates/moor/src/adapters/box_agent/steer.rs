//! One steer: text typed and submitted, or a menu choice, into the agent's pane.

use std::time::Duration;

use moor_core::seam::agent_adapter::{AgentAdapter, Key as AgentKey};
use moor_core::seam::clock::Clock;
use moor_core::seam::engine::{EngineError, SessionRef, Steer, SteerOutcome};
use moor_core::seam::proc::Deadline;
use moor_core::seam::surface::{Key, PaneRef, Surface, SurfaceError};

/// Time for the pane to take the typed text, and then the submit key.
const SUBMIT_DELAY: Duration = Duration::from_millis(300);
/// How many trailing lines of the pane show the input after submitting.
const CHECK_LINES: usize = 12;
/// How many trailing characters of the text show that it is still in the input.
const TAIL_CHARS: usize = 24;

/// Sends `steer` to the agent's pane; the text is typed once.
pub(super) fn steer(
    surface: &dyn Surface,
    profile: &dyn AgentAdapter,
    clock: &dyn Clock,
    s: &SessionRef,
    steer: &Steer,
    deadline: Deadline,
) -> Result<SteerOutcome, EngineError> {
    let pane = s.pane.as_ref().ok_or(EngineError::NoSession)?;
    if ended(surface, pane, deadline)? {
        return Err(EngineError::PaneGone);
    }
    match steer {
        Steer::Menu(n) => {
            let keys: Vec<Key> = profile
                .menu_keys(*n)
                .into_iter()
                .map(pane_key)
                .collect::<Option<_>>()
                .ok_or_else(|| failed("menu", "the choice has no key".to_owned()))?;
            send(surface.send_keys(pane, &keys), "menu")?;
            Ok(SteerOutcome {
                submit_retried: false,
                submit_checked: false,
            })
        }
        Steer::Interrupt => {
            let key = pane_key(profile.interrupt_key()).ok_or_else(|| {
                failed("interrupt", "the interrupt key has no pane key".to_owned())
            })?;
            send(surface.send_keys(pane, &[key]), "interrupt")?;
            Ok(SteerOutcome {
                submit_retried: false,
                submit_checked: false,
            })
        }
        Steer::Text(text) => {
            send(surface.send_keys(pane, &[Key::CtrlU]), "clear")?;
            send(surface.send_text(pane, text), "type")?;
            clock.sleep(SUBMIT_DELAY);
            send(surface.send_keys(pane, &[Key::CtrlM]), "submit")?;
            clock.sleep(SUBMIT_DELAY);
            let (retry, checked) = match surface.pane_obs(pane, deadline) {
                Ok(obs) if obs.exit_marker.is_some() => return Err(EngineError::PaneGone),
                Ok(obs) => (
                    submit_pending(&last_lines(&obs.tail), text, profile.busy_marker()),
                    true,
                ),
                Err(SurfaceError::NotFound) => return Err(EngineError::PaneGone),
                Err(_) => (false, false),
            };
            if retry {
                send(surface.send_keys(pane, &[Key::CtrlM]), "submit")?;
            }
            Ok(SteerOutcome {
                submit_retried: retry,
                submit_checked: checked,
            })
        }
    }
}

/// Whether the pane is gone or shows that the agent exited.
fn ended(surface: &dyn Surface, pane: &PaneRef, deadline: Deadline) -> Result<bool, EngineError> {
    match surface.pane_obs(pane, deadline) {
        Ok(obs) => Ok(obs.exit_marker.is_some()),
        Err(SurfaceError::NotFound) => Ok(true),
        Err(e) => Err(failed("read", e.to_string())),
    }
}

/// A send's result as an Engine error; a missing pane is `PaneGone`.
fn send(r: Result<(), SurfaceError>, step: &'static str) -> Result<(), EngineError> {
    r.map_err(|e| match e {
        SurfaceError::NotFound => EngineError::PaneGone,
        e => failed(step, e.to_string()),
    })
}

/// A failed `step` as an Engine error.
const fn failed(step: &'static str, message: String) -> EngineError {
    EngineError::Failed { step, message }
}

/// The pane key for an agent key; `None` for a character that is not a digit.
fn pane_key(key: AgentKey) -> Option<Key> {
    match key {
        AgentKey::Char(c) => u8::try_from(c.to_digit(10)?).ok().map(Key::Digit),
        AgentKey::Escape => Some(Key::Escape),
    }
}

/// The last [`CHECK_LINES`] lines of `tail`.
fn last_lines(tail: &str) -> String {
    let mut lines: Vec<&str> = tail.lines().rev().take(CHECK_LINES).collect();
    lines.reverse();
    lines.join("\n")
}

/// Whether the end of `text` is still in the pane's input and the agent shows no busy marker.
fn submit_pending(tail: &str, text: &str, busy: &str) -> bool {
    if tail.to_lowercase().contains(busy) {
        return false;
    }
    let squash = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
    let text = squash(text);
    let skip = text.chars().count().saturating_sub(TAIL_CHARS);
    let end: String = text.chars().skip(skip).collect();
    !end.is_empty() && squash(tail).contains(&end)
}

#[cfg(test)]
mod tests;
