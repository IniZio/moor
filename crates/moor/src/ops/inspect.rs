//! `inspect`: a Box's pane tail, the agent's recent messages and why it looks stuck. It reads
//! and never settles, so looking at a Box changes nothing.

use moor_core::view::progress::Happened;
use moor_core::view::stuck::stuck_of;
use moor_core::view::{InspectView, PaneView, SaidView};
use moor_core::{ErrorKind, InspectReq};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::activity;
use crate::ops::common::target::{self, Rule};
use crate::ops::common::view::{agent_view, rfc3339};

/// Pane lines returned when the request names none, and the most it may ask for.
const PANE_LINES: u32 = 40;
/// See [`PANE_LINES`].
const MAX_PANE_LINES: u32 = 200;
/// Agent messages returned when the request names none, and the most it may ask for.
const SAID: u32 = 5;
/// See [`SAID`].
const MAX_SAID: u32 = 20;
/// Events read to decide whether the Box looks stuck.
const EVENT_WINDOW: usize = 40;

/// Widen a wanted count to the machine's size.
fn count(wanted: Option<u32>, default: u32, max: u32) -> usize {
    usize::try_from(wanted.unwrap_or(default).min(max)).unwrap_or(usize::MAX)
}

/// The last `lines` lines of the pane; none without an observation.
fn pane_tail(reading: &activity::Reading, lines: usize) -> Option<PaneView> {
    let text = reading.pane_text.as_deref()?;
    let skip = text.lines().count().saturating_sub(lines);
    Some(PaneView {
        tail: text.lines().skip(skip).collect::<Vec<_>>().join("\n"),
        status: reading.pane.as_str().to_owned(),
    })
}

/// Run `inspect`.
pub(crate) fn run(cx: &Cx, req: &InspectReq) -> Result<InspectView, CommandError> {
    let store = cx.store()?;
    let target = &req.target;
    let row = target::resolve(store, target, Rule::Latest)?
        .ok_or_else(|| CommandError::new(ErrorKind::Failed, format!("no Box {target}")))?;
    let reading = activity::read(cx, store, &row, cx.clock().now(), activity::PROBE_DEADLINE)?;
    let said = store
        .newest_events_of(row.id, "agent_said", count(req.said, SAID, MAX_SAID))?
        .into_iter()
        .filter_map(|e| {
            Some(SaidView {
                text: e.payload.get("text")?.as_str()?.to_owned(),
                at: rfc3339(e.ts),
            })
        })
        .collect();
    let happened: Vec<Happened> = store
        .events(row.id, EVENT_WINDOW)?
        .into_iter()
        .map(|e| Happened {
            kind: e.kind,
            payload: e.payload,
            ts: rfc3339(e.ts),
        })
        .collect();
    Ok(InspectView {
        box_name: row.name.as_str().to_owned(),
        lifecycle: row.lifecycle.as_str().to_owned(),
        session: row.session.as_str().to_owned(),
        agent: agent_view(&reading),
        pane: pane_tail(&reading, count(req.lines, PANE_LINES, MAX_PANE_LINES)),
        said,
        stuck: stuck_of(&happened),
    })
}

#[cfg(test)]
mod tests;
