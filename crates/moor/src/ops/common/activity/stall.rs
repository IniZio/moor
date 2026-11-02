//! The `agent_stalled` and `agent_resumed` events, written once per stall.

use moor_core::activity::{stalled_for, HookKind};
use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::seam::clock::Ts;

use super::Reading;
use crate::error::CommandError;
use crate::store::{BoxRow, Store};

/// Append `agent_stalled` when the agent's hook reported busy and has gone quiet, and
/// `agent_resumed` when a report arrives after one.
///
/// A run in flight keeps the agent busy by itself, so it is never a stall.
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
    let quiet = (record.kind == HookKind::Busy)
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
