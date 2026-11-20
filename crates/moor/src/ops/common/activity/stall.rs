//! A working agent gone quiet: the `agent_stalled` and `agent_resumed` events, written once per
//! stall, and a wait on background work that is gone.

use moor_core::activity::{
    stalled_for, wait_overdue, Activity, AgentState, Probe, Source, Status, WaitCheck,
};
use moor_core::domain::decision::ExtraEvent;
use moor_core::seam::clock::Ts;
use moor_core::seam::engine::SessionRef;
use moor_core::seam::proc::Deadline;
use moor_core::workflow::HistoryEntry;
use moor_core::BoxId;

use super::{apply_read, Reading};
use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::{BoxRow, Store};

/// `activity` without the background work the Box shows to be gone: no hook fires when background
/// work ends without waking the agent. An agent that only waited on it is idle; one that asked
/// the person still asks. Only work the Box can see is checked.
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
        let state = match activity.state {
            AgentState::Asked { question, .. } => AgentState::Asked {
                question,
                background: Vec::new(),
            },
            _ => AgentState::Idle,
        };
        return Activity { state, ..activity };
    }
    activity
}

/// Append `agent_stalled` when the agent has reported working with no sign of activity for the
/// stall bound, and `agent_resumed` at the first sign after one. A sign is a change of its status,
/// a new doing line or a step report.
///
/// A run in flight keeps the agent busy by itself, so it is never a stall. Neither is a Box whose
/// agent could not be read: the agent behind a broken probe may be fine.
pub(super) fn note_stall(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    reading: &Reading,
    now: Ts,
) -> Result<(), CommandError> {
    if reading.run_in_flight {
        return Ok(());
    }
    let Some((term, status_at)) = reading
        .term
        .as_ref()
        .and_then(|t| t.status_at.map(|at| (t, at)))
    else {
        return Ok(());
    };
    let stalled = store.live_launch(row.id)?.and_then(|l| l.stalled_at);
    let open = stalled.is_some();
    let working = matches!(
        reading.activity,
        Some(Activity {
            state: AgentState::Working,
            source: Source::Status,
        })
    );
    if !working && !open {
        return Ok(());
    }
    let at = last_active(store, row.id, status_at)?;
    let doing = match &term.status {
        Some(Status::Working { doing }) => doing.clone(),
        _ => None,
    };
    let quiet = working.then(|| stalled_for(at, now)).flatten();
    match (quiet, stalled) {
        (Some(quiet_secs), _) if !open => {
            let stalled = ExtraEvent::AgentStalled { doing, quiet_secs };
            apply_read(cx, store, reading.decision(row.id).event(stalled))?;
        }
        (None, Some(s)) if at > s => {
            // The quiet before the stall is only reported, so its event is read for the number.
            let before = store
                .newest_event_of(row.id, "agent_stalled")?
                .and_then(|e| e.payload.get("quiet_secs")?.as_u64());
            let since = u64::try_from((at.as_millis() - s.as_millis()) / 1000).unwrap_or(0);
            let resumed = ExtraEvent::AgentResumed {
                quiet_secs: before.unwrap_or(0) + since,
            };
            apply_read(cx, store, reading.decision(row.id).event(resumed))?;
        }
        _ => {}
    }
    Ok(())
}

/// When Box `id`'s agent last showed activity: its status changed at `status_at`, its doing line
/// last moved, or it last reported a step of the run it works on. A long turn keeps one status.
fn last_active(store: &Store, id: BoxId, status_at: Ts) -> Result<Ts, CommandError> {
    let doing = store.last_event_of(id, "agent_doing")?.map(|e| e.ts);
    let reported = match store.running_run_of_box(id)? {
        Some((_, run)) => store
            .run_entries(run.id)?
            .into_iter()
            .rev()
            .find(|(_, e)| {
                matches!(
                    e,
                    HistoryEntry::Reported { .. } | HistoryEntry::GateRefused { .. }
                )
            })
            .map(|(at, _)| at),
        None => None,
    };
    Ok([doing, reported]
        .into_iter()
        .flatten()
        .fold(status_at, Ts::max))
}
