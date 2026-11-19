//! A Box whose agent no read reaches: `agent_unreachable` once its reads have failed for a while,
//! and `agent_link_restored` at the first read it answers again. Neither ends the turn or the
//! session: the Box keeps its error until a read answers or a person acts on it.

use std::time::Duration;

use moor_core::activity::Probe;
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::seam::clock::Ts;

use super::apply_read;
use crate::cx::owners::Cmd;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::{BoxRow, Launch, Store};

/// How long the reads of an agent may fail before its Box counts as unreachable.
const UNREACHABLE_AFTER: Duration = Duration::from_secs(120);

/// Notes whether `probe`, a read of `launch`, reached the agent of `row`.
///
/// The bound counts from the last answer, or from the launch when none came, so a controller
/// restart cannot hide an agent that went silent. Only a Box that is up and that no op is moving
/// counts: an asleep Box answers nothing by design.
pub(super) fn note_read(
    cx: &Cx,
    store: &Store,
    (row, launch): (&BoxRow, &Launch),
    probe: &Probe,
    now: Ts,
) -> Result<(), CommandError> {
    let answered = *probe != Probe::TimedOut;
    if answered {
        store.note_agent_read(launch.id, now)?;
    }
    let restored = answered && launch.unreachable_since.is_some();
    let moving = row.lifecycle != BoxLifecycle::Ready || row.effect.is_some() || row.asleep();
    let marked = launch.unreachable_since.is_some() && !answered;
    if marked || (!restored && (answered || moving)) {
        return Ok(());
    }
    // Two reads of one Box may overlap, so its launch is read again under its turn.
    let Some(_turn) = cx.try_turn(row.id, Cmd::Settle) else {
        return Ok(());
    };
    let since = store.launch(launch.id)?.and_then(|l| l.unreachable_since);
    let event = |e| Decision::new(row.id).launch(launch.id).event(e);
    if restored {
        if since.is_some() {
            let pane = launch.session_ref.clone();
            apply_read(cx, store, event(ExtraEvent::AgentLinkRestored { pane }))?;
        }
        return Ok(());
    }
    let last = store.agent_read_at(launch.id)?.unwrap_or(launch.started_at);
    let silent = elapsed(last, now);
    if since.is_none() && silent >= UNREACHABLE_AFTER {
        let error = format!(
            "the agent has not answered a read for {} s",
            silent.as_secs()
        );
        let unreachable = ExtraEvent::AgentUnreachable { attempts: 1, error };
        apply_read(cx, store, event(unreachable))?;
    }
    Ok(())
}

/// The time from `from` to `now`, zero when the clock is behind.
fn elapsed(from: Ts, now: Ts) -> Duration {
    let ms = now.as_millis().saturating_sub(from.as_millis());
    Duration::from_millis(u64::try_from(ms).unwrap_or(0))
}
