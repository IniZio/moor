//! Watches a fresh agent pane for a first-run prompt or an agent that died at launch.

use std::time::Duration;

use moor_core::activity::{Life, Probe, Term};
use moor_core::domain::decision::{Decision, ExtraEvent, Field};
use moor_core::exec_env::prelude_stop;
use moor_core::seam::proc::Deadline;
use moor_core::view::{AgentLaunchView, LaunchFailure};
use moor_core::{BoxId, Warning};

use super::launch::{record, to_json, TAIL_LINES};
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::activity::session_ref;
use crate::ops::common::profile_fetch;
use crate::ops::common::provision::reread;
use crate::ops::common::view::rfc3339;
use crate::ops::common::wake::tell_relaunched;
use crate::store::Store;

/// Time between two reads of the pane while watching.
const PROMPT_POLL: Duration = Duration::from_millis(500);

/// Pane lines, from the end, that the watch matches against.
const PROMPT_LINES: usize = 40;

/// Deadline of one pane read while watching.
const PROMPT_READ: Duration = Duration::from_secs(5);

/// Recent events searched for a lost disk.
const LOST_DISK_EVENTS: usize = 100;

/// Tells the agent just launched, if it is the first since its Box's disk was lost, what is gone.
/// Best effort: the owner already has the `disk_lost` event.
fn tell_of_lost_disk(cx: &Cx, store: &Store, id: BoxId) {
    let first = store.events(id, LOST_DISK_EVENTS).is_ok_and(|events| {
        let after = events.iter().rev().take_while(|e| e.kind != "disk_lost");
        let launches = after.filter(|e| e.kind == "session_started").count();
        launches == 1 && events.iter().any(|e| e.kind == "disk_lost")
    });
    if !first {
        return;
    }
    let told = reread(store, id).and_then(|row| {
        let text = "This Box's machine was lost and a new one replaced it, so its disk is new: \
                    files and commits not pushed, and every process, are gone. Check what is \
                    there before you carry on.";
        crate::ops::steer::tell(cx, store, &row, text.to_owned())
    });
    if let Err(e) = told {
        cx.warn(Warning {
            code: "note_failed".to_owned(),
            message: format!("cannot tell the agent its disk was lost: {}", e.error),
        });
    }
}

/// Watches the fresh agent pane and reports a launch whose agent died at once as failed.
///
/// An exit marker in the pane ends the live session and records the failure like any other
/// launch failure; the returned view is the failed launch. Otherwise `agent` comes back as is.
pub(crate) fn run(cx: &Cx, store: &Store, id: BoxId, agent: AgentLaunchView) -> AgentLaunchView {
    if !agent.launched {
        return agent;
    }
    let Some((session, code, tail)) = poll(cx, store, id) else {
        report_installs(cx, store, id);
        tell_of_lost_disk(cx, store, id);
        if let Err(e) = tell_relaunched(cx, store, id) {
            cx.warn(Warning {
                code: "note_failed".to_owned(),
                message: format!("cannot tell the agent it was started again: {}", e.error),
            });
        }
        return agent;
    };
    let failure = exited(cx, code, &tail);
    let recorded = store
        .end_session(session, code, None)
        .map_err(CommandError::from)
        .and_then(|()| record(store, id, Field::Set(to_json(&failure)?), Some(&failure)));
    if let Err(e) = recorded {
        warn_record(cx, &e.error);
    }
    AgentLaunchView {
        launched: false,
        session: None,
        reason: Some(failure),
        nonce: None,
    }
}

/// Records the profile installs the launch script reports as failed; a store failure is warned.
fn report_installs(cx: &Cx, store: &Store, id: BoxId) {
    let reported = store
        .box_row(id)
        .map_err(CommandError::from)
        .and_then(|row| row.map_or(Ok(()), |row| profile_fetch::installs(cx, store, &row)));
    if let Err(e) = reported {
        cx.warn(Warning {
            code: "profile_check_failed".to_owned(),
            message: format!(
                "could not record which profile installs failed: {}",
                e.error
            ),
        });
    }
}

/// The failure for an agent that exited with `code` right after launch. A launch the prelude
/// stopped, because the Proxy CA could not be written or the egress proxy could not start,
/// carries the prelude's message.
fn exited(cx: &Cx, code: Option<i32>, tail: &str) -> LaunchFailure {
    let message = match prelude_stop(tail) {
        Some(m) => m.to_owned(),
        None if code == Some(127) => {
            "the agent exited 127: its command was not found on the Box".to_owned()
        }
        None => code.map_or_else(
            || "the agent was ended by a signal right after launch".to_owned(),
            |code| format!("the agent exited {code} right after launch"),
        ),
    };
    let tail = last_lines(tail, TAIL_LINES);
    LaunchFailure {
        step: "launch".to_owned(),
        message,
        tail: Some(tail).filter(|t| !t.trim().is_empty()),
        at: rfc3339(cx.clock().now()),
    }
}

/// The event for a first-run screen `screen` shows, warned about; none when it shows none.
fn blocked(cx: &Cx, id: BoxId, pane: &str, screen: Option<&str>) -> Option<ExtraEvent> {
    let m = cx
        .engine()
        .first_run_prompt(&last_lines(screen.unwrap_or(""), PROMPT_LINES))?;
    let name = m.prompt.name();
    cx.warn(Warning {
        code: "agent_prompt_blocked".to_owned(),
        message: format!(
            "the agent of Box {} stopped at its {name} screen (\"{}\"); moor's pre-seed did not cover it, which is a moor bug",
            id.get(),
            m.line
        ),
    });
    Some(ExtraEvent::AgentPromptBlocked {
        pane: pane.to_owned(),
        prompt: name.to_owned(),
        line: m.line,
    })
}

/// Watches the agent just launched until it reports a status, a first-run screen holds it, or
/// the prompt window closes; the session, exit code and screen of an agent that ended meanwhile.
fn poll(cx: &Cx, store: &Store, id: BoxId) -> Option<(i64, Option<i32>, String)> {
    let live = match store.live_session(id) {
        Ok(Some(row)) => row,
        Ok(None) => return None,
        Err(e) => {
            warn_record(cx, &e);
            return None;
        }
    };
    let row = store.box_row(id).ok().flatten()?;
    let sref = session_ref(&row, &live)?;
    let pane = live.pane.clone().unwrap_or_default();
    let begin = cx.clock().instant();
    loop {
        let event = match cx.engine().probe(&sref, Deadline(PROMPT_READ)) {
            Probe::Read {
                term:
                    Some(Term {
                        life: Life::Finished { exit },
                        ..
                    }),
                screen,
                ..
            } => return Some((live.id, exit, screen.unwrap_or_default())),
            Probe::Read { term: None, .. }
            | Probe::Read {
                term: Some(Term {
                    status: Some(_), ..
                }),
                ..
            } => return None,
            Probe::Read { screen, .. } => blocked(cx, id, &pane, screen.as_deref()),
            Probe::TimedOut => Some(ExtraEvent::AgentPromptCheckFailed {
                pane: pane.clone(),
                error: "the agent could not be read".to_owned(),
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
