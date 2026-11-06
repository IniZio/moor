//! Hook reports gone quiet: the `agent_stalled` and `agent_resumed` events, written once per
//! stall, and a wait on background work that is gone.

use moor_core::activity::{
    stalled_for, wait_overdue, Activity, AgentState, HookKind, HookRead, Probes, Source, WaitCheck,
};
use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::seam::clock::Ts;
use moor_core::seam::engine::SessionRef;
use moor_core::seam::proc::Deadline;

use super::Reading;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::{BoxRow, Store};

/// `probes` with any report the Box or its pane shows to be over rewritten as idle.
pub(super) fn end_stale(
    cx: &Cx,
    sref: &SessionRef,
    probes: Probes,
    now: Ts,
    deadline: Deadline,
) -> Probes {
    let probes = end_wait(cx, sref, probes, now, deadline);
    super::needs_input::end_gone_prompt(cx, probes, now)
}

/// `probes` with an overdue wait the Box shows to be empty rewritten as the idle report the agent
/// never wrote: no hook fires when background work ends without waking the agent.
fn end_wait(cx: &Cx, sref: &SessionRef, mut probes: Probes, now: Ts, deadline: Deadline) -> Probes {
    if let HookRead::Fresh(rec) = &mut probes.hook {
        if wait_overdue(rec, now) && cx.engine().check_wait(sref, deadline) == WaitCheck::Nothing {
            rec.kind = HookKind::Idle;
        }
    }
    probes
}

/// Append `agent_stalled` when the agent's hook reported busy and has gone quiet, and
/// `agent_resumed` when a report arrives after one.
///
/// A run in flight keeps the agent busy by itself, so it is never a stall. Neither is a Box whose
/// hook could not be read: the agent behind a broken probe may be fine.
pub(super) fn note_stall(
    store: &Store,
    row: &BoxRow,
    reading: &Reading,
    now: Ts,
) -> Result<(), CommandError> {
    if reading.run_in_flight {
        return Ok(());
    }
    let Some(cache) = store.hook(row.id)? else {
        return Ok(());
    };
    let record = cache.record;
    let stalled = store.newest_event_of(row.id, "agent_stalled")?;
    let open = match &stalled {
        Some(s) => store
            .newest_event_of(row.id, "agent_resumed")?
            .is_none_or(|r| r.id < s.id),
        None => false,
    };
    let hooked = matches!(
        reading.activity,
        Some(Activity {
            state: AgentState::Working,
            source: Source::Hooks | Source::HooksCached,
        })
    );
    let quiet = (hooked && record.kind == HookKind::Busy)
        .then(|| stalled_for(record.at, now))
        .flatten();
    match (quiet, stalled) {
        (Some(quiet_secs), _) if !open => {
            store.apply(Decision::new(row.id).event(ExtraEvent::AgentStalled {
                doing: record.doing,
                quiet_secs,
            }))?;
        }
        (None, Some(s)) if open && record.at > s.ts => {
            let before = s
                .payload
                .get("quiet_secs")
                .and_then(serde_json::Value::as_u64);
            let since =
                u64::try_from((record.at.as_millis() - s.ts.as_millis()) / 1000).unwrap_or(0);
            store.apply(Decision::new(row.id).event(ExtraEvent::AgentResumed {
                quiet_secs: before.unwrap_or(0) + since,
            }))?;
        }
        _ => {}
    }
    Ok(())
}
