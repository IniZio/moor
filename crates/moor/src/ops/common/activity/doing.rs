//! The `agent_doing` event: what the agent is doing now, written when the line changes.

use std::time::Duration;

use moor_core::activity::HookRecord;
use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::seam::clock::Ts;
use moor_core::BoxId;

use crate::store::{Store, StoreError};

/// Least time between two `agent_doing` events of one Box, so a busy agent does not flood
/// the log; a line that changed meanwhile is written on the first read after it.
pub(crate) const DOING_EVERY: Duration = Duration::from_secs(30);

/// Writes `agent_doing` when `rec` names a line other than the Box's last one and the last
/// one is at least [`DOING_EVERY`] old at `now`.
///
/// # Errors
/// A store failure.
pub(super) fn note_doing(
    store: &Store,
    id: BoxId,
    rec: &HookRecord,
    now: Ts,
) -> Result<(), StoreError> {
    let Some(text) = rec.doing.clone() else {
        return Ok(());
    };
    if let Some(last) = store.last_event_of(id, "agent_doing")? {
        let every = i64::try_from(DOING_EVERY.as_millis()).unwrap_or(i64::MAX);
        if last.payload.get("text").and_then(serde_json::Value::as_str) == Some(text.as_str())
            || now.as_millis() - last.ts.as_millis() < every
        {
            return Ok(());
        }
    }
    store.apply(Decision::new(id).event(ExtraEvent::AgentDoing { text }))?;
    Ok(())
}
