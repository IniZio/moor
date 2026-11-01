//! An agent pane that ended may have lost only its link: the agent can outlive it on the Box.

use std::time::Duration;

use moor_core::activity::AgentState;
use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::seam::engine::AgentFate;
use moor_core::seam::proc::Deadline;
use moor_core::seam::surface::{PaneRef, WorkspaceRef};
use moor_core::BoxId;

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::activity::{session_ref, Reading};
use crate::ops::common::boxlock::BoxLock;
use crate::store::{BoxRow, SessionRow, Store};

/// Tries at reaching the Box before its agent counts as ended; the waits double from 1 s.
const TRIES: u32 = 4;
/// Time allowed for each try.
const TRY_DEADLINE: Duration = Duration::from_secs(20);
/// How long a Box may stay unreachable, over later tries, before its agent counts as ended.
const UNREACHABLE_FOR: Duration = Duration::from_secs(300);
/// Recent events searched for when the Box first went unreachable.
const RECENT_EVENTS: usize = 200;

/// Whether the session of an ended pane goes on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Fate {
    /// A new pane attached to the agent, which still runs.
    Reattached,
    /// The agent ended, with this exit status.
    Ended(Option<i32>),
    /// The Box did not answer; a later try decides.
    Unreachable,
}

/// Asks the Box what became of the agent of `launch`, whose pane ended with `pane_exit`, and
/// attaches a new pane when it still runs. Each round of tries the Box does not answer gets an
/// `agent_unreachable` event; the agent counts as ended once the Box has not answered for
/// `UNREACHABLE_FOR`. The caller holds the Box lock.
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
    let Some(sref) = session_ref(row, launch.session_ref.clone(), launch.pane.clone()) else {
        return Ok(Fate::Ended(pane_exit));
    };
    let mut attempt = 0;
    loop {
        match cx.engine().agent_fate(&sref, Deadline(TRY_DEADLINE)) {
            Ok(AgentFate::Running) => break,
            Ok(AgentFate::Ended { exit }) => return Ok(Fate::Ended(exit.or(pane_exit))),
            Err(e) => {
                attempt += 1;
                if attempt == TRIES {
                    let since = unreachable_since(store, row.id, launch)?;
                    let error = e.to_string();
                    let event = ExtraEvent::AgentUnreachable {
                        attempts: TRIES,
                        error,
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
    let Some(ws) = row.workspace_id.as_deref().map(WorkspaceRef::new) else {
        return Ok(Fate::Ended(pane_exit));
    };
    let Ok(pane) = cx.engine().reattach(&sref, &ws, &row.name) else {
        return Ok(Fate::Ended(pane_exit));
    };
    store.reattach_pane(row.id, launch.id, pane_exit, pane.as_str())?;
    if let Some(old) = launch.pane.as_deref().filter(|old| *old != pane.as_str()) {
        let _ = cx.surface().close_pane(&PaneRef::new(old.to_owned()));
    }
    Ok(Fate::Reattached)
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

/// What became of the agent whose pane `reading` saw end, or `None` when this settle may not
/// tell: the session is not the probed one, or another operation holds the Box lock.
pub(crate) fn after_pane_end(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    reading: &Reading,
    probed: i64,
) -> Result<Option<Fate>, CommandError> {
    let Some(launch) = store.live_session(row.id)?.filter(|s| s.id == probed) else {
        return Ok(None);
    };
    let Ok(_held) = BoxLock::acquire(
        cx.clock(),
        cx.state_dir()?,
        row.id,
        "session end",
        Duration::ZERO,
    ) else {
        return Ok(None);
    };
    let pane_exit = match reading.activity.as_ref().map(|a| &a.state) {
        Some(AgentState::Exited { code }) => Some(*code),
        _ => None,
    };
    Ok(Some(recover(cx, store, row, &launch, pane_exit)?))
}
