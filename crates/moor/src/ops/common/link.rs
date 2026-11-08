//! What became of a session's agent, asked of the Box, which may not answer for a while.

use std::time::Duration;

use moor_core::activity::{Life, Probe, Term};
use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::seam::proc::Deadline;
use moor_core::BoxId;

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::activity::session_ref;
use crate::store::{BoxRow, SessionRow, Store};

/// Tries at reaching the Box before its agent counts as ended; the waits double from 1 s.
const TRIES: u32 = 4;
/// Time allowed for each try.
const TRY_DEADLINE: Duration = Duration::from_secs(20);
/// How long a Box may stay unreachable, over later tries, before its agent counts as ended.
const UNREACHABLE_FOR: Duration = Duration::from_secs(300);
/// Recent events searched for when the Box first went unreachable.
const RECENT_EVENTS: usize = 200;

/// What became of a session's agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Fate {
    /// The agent still runs.
    Running,
    /// The agent ended, with this exit status.
    Ended(Option<i32>),
    /// The Box did not answer; a later try decides.
    Unreachable,
}

/// Asks the Box what became of the agent of `launch`, `pane_exit` being the exit already seen.
/// Each round of tries the Box does not answer gets an
/// `agent_unreachable` event; the agent counts as ended once the Box has not answered for
/// `UNREACHABLE_FOR`. A suspended VM keeps its agent and cannot be asked, so an asleep Box decides
/// nothing until a wake asks it. The caller holds the Box lock.
///
/// # Errors
/// A store failure.
pub(crate) fn recover(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    launch: &SessionRow,
    pane_exit: Option<i32>,
) -> Result<Fate, CommandError> {
    if row.asleep() {
        return Ok(Fate::Unreachable);
    }
    let Some(sref) = session_ref(row, launch) else {
        return Ok(Fate::Ended(pane_exit));
    };
    store.require_hc_holder(row.id, row.name.as_str())?;
    let mut attempt = 0;
    loop {
        match cx.engine().probe(&sref, Deadline(TRY_DEADLINE)) {
            Probe::Read {
                term: Some(Term {
                    life: Life::Live, ..
                }),
                ..
            } => return Ok(Fate::Running),
            Probe::Read {
                term:
                    Some(Term {
                        life: Life::Finished { exit },
                        ..
                    }),
                ..
            } => return Ok(Fate::Ended(exit.or(pane_exit))),
            Probe::Read { term: None, .. } => return Ok(Fate::Ended(pane_exit)),
            Probe::TimedOut => {
                attempt += 1;
                if attempt == TRIES {
                    let since = unreachable_since(store, row.id, launch)?;
                    let event = ExtraEvent::AgentUnreachable {
                        attempts: TRIES,
                        error: "the agent could not be read".to_owned(),
                    };
                    store.apply(Decision::new(row.id).event(event))?;
                    let bound = i64::try_from(UNREACHABLE_FOR.as_millis()).unwrap_or(i64::MAX);
                    let now = cx.clock().now().as_millis();
                    return Ok(match since {
                        Some(t) if now.saturating_sub(t) >= bound => Fate::Ended(pane_exit),
                        _ => Fate::Unreachable,
                    });
                }
                cx.clock().sleep(Duration::from_secs(1 << (attempt - 1)));
            }
        }
    }
}

/// When, in ms, the Box of `launch` first failed to answer since the launch began or its link
/// was last restored; none when it has answered every time.
fn unreachable_since(
    store: &Store,
    id: BoxId,
    launch: &SessionRow,
) -> Result<Option<i64>, CommandError> {
    let started = launch.started_at.as_millis();
    Ok(store
        .events(id, RECENT_EVENTS)?
        .iter()
        .rev()
        .take_while(|e| e.kind != "agent_link_restored" && e.ts.as_millis() >= started)
        .filter(|e| e.kind == "agent_unreachable")
        .last()
        .map(|e| e.ts.as_millis()))
}
