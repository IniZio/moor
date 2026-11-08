//! A working agent gone quiet: the `agent_stalled` and `agent_resumed` events, written once per
//! stall, and a wait on background work that is gone.

use moor_core::activity::{
    stalled_for, wait_overdue, Activity, AgentState, Probe, Source, Status, WaitCheck,
};
use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::seam::clock::Ts;
use moor_core::seam::engine::SessionRef;
use moor_core::seam::proc::Deadline;

use super::Reading;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::{BoxRow, Store};

/// `activity` as idle when it waits on background work the Box shows to be gone: no hook fires
/// when background work ends without waking the agent. Only work the Box can see is checked.
pub(super) fn end_stale_wait(
    cx: &Cx,
    sref: &SessionRef,
    probe: &Probe,
    activity: Activity,
    now: Ts,
    deadline: Deadline,
) -> Activity {
    let (Some(term), Some(note)) = (probe.term(), probe.note()) else {
        return activity;
    };
    if note.background_local
        && wait_overdue(&activity.state, term, now)
        && cx.engine().check_wait(sref, deadline) == WaitCheck::Nothing
    {
        return Activity {
            state: AgentState::Idle,
            ..activity
        };
    }
    activity
}

/// Append `agent_stalled` when the agent has reported working with no change for the stall bound,
/// and `agent_resumed` when its status changes after one.
///
/// A run in flight keeps the agent busy by itself, so it is never a stall. Neither is a Box whose
/// agent could not be read: the agent behind a broken probe may be fine.
pub(super) fn note_stall(
    store: &Store,
    row: &BoxRow,
    reading: &Reading,
    now: Ts,
) -> Result<(), CommandError> {
    if reading.run_in_flight {
        return Ok(());
    }
    let Some((term, at)) = reading
        .term
        .as_ref()
        .and_then(|t| t.status_at.map(|at| (t, at)))
    else {
        return Ok(());
    };
    let stalled = store.newest_event_of(row.id, "agent_stalled")?;
    let open = match &stalled {
        Some(s) => store
            .newest_event_of(row.id, "agent_resumed")?
            .is_none_or(|r| r.id < s.id),
        None => false,
    };
    let working = matches!(
        reading.activity,
        Some(Activity {
            state: AgentState::Working,
            source: Source::Status,
        })
    );
    let doing = match &term.status {
        Some(Status::Working { doing }) => doing.clone(),
        _ => None,
    };
    let quiet = working.then(|| stalled_for(at, now)).flatten();
    match (quiet, stalled) {
        (Some(quiet_secs), _) if !open => {
            store.apply(
                Decision::new(row.id).event(ExtraEvent::AgentStalled { doing, quiet_secs }),
            )?;
        }
        (None, Some(s)) if open && at > s.ts => {
            let before = s
                .payload
                .get("quiet_secs")
                .and_then(serde_json::Value::as_u64);
            let since = u64::try_from((at.as_millis() - s.ts.as_millis()) / 1000).unwrap_or(0);
            store.apply(Decision::new(row.id).event(ExtraEvent::AgentResumed {
                quiet_secs: before.unwrap_or(0) + since,
            }))?;
        }
        _ => {}
    }
    Ok(())
}
