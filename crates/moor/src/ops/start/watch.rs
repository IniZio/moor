//! Watches a fresh agent pane for a first-run prompt or an agent that died at launch.

use std::time::Duration;

use moor_core::domain::decision::{Decision, ExtraEvent, Field};
use moor_core::seam::proc::Deadline;
use moor_core::seam::surface::{PaneRef, SurfaceError};
use moor_core::view::{AgentLaunchView, LaunchFailure};
use moor_core::{TaskId, Warning};

use super::launch::{record, to_json, TAIL_LINES};
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::view::rfc3339;
use crate::store::Store;

/// Time between two reads of the pane while watching.
const PROMPT_POLL: Duration = Duration::from_millis(500);

/// Pane lines, from the end, that the watch matches against.
const PROMPT_LINES: usize = 40;

/// Deadline of one pane read while watching.
const PROMPT_READ: Duration = Duration::from_secs(5);

/// Watches the fresh agent pane and reports a launch whose agent died at once as failed.
///
/// An exit marker in the pane ends the live session and records the failure like any other
/// launch failure; the returned view is the failed launch. Otherwise `agent` comes back as is.
pub(super) fn run(cx: &Cx, store: &Store, id: TaskId, agent: AgentLaunchView) -> AgentLaunchView {
    if !agent.launched {
        return agent;
    }
    let Some((session, code, tail)) = poll(cx, store, id) else {
        return agent;
    };
    let failure = exited(cx, code, &tail);
    let recorded = store
        .end_session(session, Some(code), None)
        .map_err(CommandError::from)
        .and_then(|()| record(store, id, Field::Set(to_json(&failure)?), Some(&failure)));
    if let Err(e) = recorded {
        warn_record(cx, &e.error);
    }
    AgentLaunchView {
        launched: false,
        session: None,
        sandbox: None,
        reason: Some(failure),
    }
}

/// The failure for an agent that exited with `code` right after launch.
fn exited(cx: &Cx, code: i32, tail: &str) -> LaunchFailure {
    let message = if code == 127 {
        "the agent exited 127: its command was not found on the Box".to_owned()
    } else {
        format!("the agent exited {code} right after launch")
    };
    let tail = last_lines(tail, TAIL_LINES);
    LaunchFailure {
        step: "launch".to_owned(),
        message,
        tail: Some(tail).filter(|t| !t.trim().is_empty()),
        at: rfc3339(cx.clock().now()),
    }
}

/// Reads the fresh agent pane for a bounded window and records a first-run prompt.
///
/// A prompt becomes an `agent_prompt_blocked` event and a warning that quotes its line; a
/// failed read becomes an `agent_prompt_check_failed` event. A pane that is gone ends the
/// watch quietly. An exit marker returns the live session id, the exit code and the pane tail.
fn poll(cx: &Cx, store: &Store, id: TaskId) -> Option<(i64, i32, String)> {
    let live = match store.live_session(id) {
        Ok(Some(row)) => row,
        Ok(None) => return None,
        Err(e) => {
            warn_record(cx, &e);
            return None;
        }
    };
    let pane = live.pane?;
    let pane_ref = PaneRef::new(pane.clone());
    let begin = cx.clock().instant();
    loop {
        let event = match cx.surface().pane_obs(&pane_ref, Deadline(PROMPT_READ)) {
            Ok(obs) => {
                if let Some(code) = obs.exit_marker {
                    return Some((live.id, code, obs.tail));
                }
                cx.engine()
                    .first_run_prompt(&last_lines(&obs.tail, PROMPT_LINES))
                    .map(|m| {
                        let name = m.prompt.name();
                        cx.warn(Warning {
                            code: "agent_prompt_blocked".to_owned(),
                            message: format!(
                                "the agent in pane {pane} waits at a {name} prompt: \"{}\"; answer it in the pane",
                                m.line
                            ),
                        });
                        ExtraEvent::AgentPromptBlocked {
                            pane: pane.clone(),
                            prompt: name.to_owned(),
                            line: m.line,
                        }
                    })
            }
            Err(SurfaceError::NotFound) => return None,
            Err(e) => Some(ExtraEvent::AgentPromptCheckFailed {
                pane: pane.clone(),
                error: e.to_string(),
            }),
        };
        if let Some(event) = event {
            if let Err(e) = store.apply(Decision::new(id).event(event)) {
                warn_record(cx, &e);
            }
            return None;
        }
        if cx.clock().instant().duration_since(begin) + PROMPT_POLL > cx.prompt_window() {
            return None;
        }
        cx.clock().sleep(PROMPT_POLL);
    }
}

/// Warns that the first-run prompt check could not be recorded.
fn warn_record(cx: &Cx, e: &dyn std::fmt::Display) {
    cx.warn(Warning {
        code: "agent_prompt_record_failed".to_owned(),
        message: format!("could not record the first-run prompt check: {e}"),
    });
}

/// The last `n` lines of `text`.
fn last_lines(text: &str, n: usize) -> String {
    let mut lines: Vec<&str> = text.lines().rev().take(n).collect();
    lines.reverse();
    lines.join("\n")
}
