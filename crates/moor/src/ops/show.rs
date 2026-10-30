//! `show`: reconcile, then report Boxes, one Box's detail and its events.
//! Each Box's agent is read once per command; a named Box reads only its own.

use std::collections::HashMap;

use moor_core::domain::boxes::BoxState;
use moor_core::seam::clock::Ts;
use moor_core::view::{
    ArtifactView, AttentionView, BoxView, EventView, HeldMachineView, MachineHoldState, PaneView,
    ReportView, SaidView, ShowView,
};
use moor_core::{BoxId, ErrorKind, ShowReq};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::artifacts::artifact_view;
use crate::ops::common::target::{self, Rule};
use crate::ops::common::view::{
    agent_view, box_view_with_run, event_view, hide_unsupported_sleep,
    hide_unsupported_sleep_cached, rfc3339,
};
use crate::ops::common::{activity, autosleep, reconcile};
use crate::store::Store;

/// Lines of the pane kept as the tail.
const PANE_LINES: usize = 40;

/// The pane section of a reading: its last lines and liveness; none without an observation.
fn pane_view(reading: &activity::Reading) -> Option<PaneView> {
    let text = reading.pane_text.as_deref()?;
    let skip = text.lines().count().saturating_sub(PANE_LINES);
    let tail = text.lines().skip(skip).collect::<Vec<_>>().join("\n");
    Some(PaneView {
        tail,
        status: reading.pane.as_str().to_owned(),
    })
}

/// The Box's open credential limits as views.
fn attention(store: &Store, box_id: BoxId) -> Result<Vec<AttentionView>, CommandError> {
    Ok(store
        .open_limits(box_id)?
        .iter()
        .map(AttentionView::from)
        .collect())
}

/// The latest version of each of the Box's artifacts as views.
fn artifacts_of(store: &Store, box_id: BoxId) -> Result<Vec<ArtifactView>, CommandError> {
    Ok(store
        .latest_artifacts(box_id)?
        .iter()
        .map(artifact_view)
        .collect())
}

/// The latest version of each of the Box's Reports as views.
fn reports_of(store: &Store, box_id: BoxId) -> Result<Vec<ReportView>, CommandError> {
    let mut latest: Vec<ReportView> = Vec::new();
    // Rows come ordered by kind then version, so the last of a kind is its newest.
    for r in store.reports(box_id)? {
        let view = ReportView {
            id: r.id,
            kind: r.kind,
            version: u32::try_from(r.version).unwrap_or(u32::MAX),
            title: r.title,
            path: r.path,
        };
        match latest.last_mut() {
            Some(last) if last.kind == view.kind => *last = view,
            _ => latest.push(view),
        }
    }
    Ok(latest)
}

/// The agent's newest message of the Box, whenever it said it.
fn said_of(store: &Store, box_id: BoxId) -> Result<Option<SaidView>, CommandError> {
    Ok(store.newest_event_of(box_id, "agent_said")?.and_then(|e| {
        Some(SaidView {
            text: e.payload.get("text")?.as_str()?.to_owned(),
            at: rfc3339(e.ts),
        })
    }))
}

/// The user machine Box `id` claims, with its slot and ports and whether it holds them;
/// `box_asleep` tells a sleeping Box from one that woke and waits for a slot.
fn held_machine(
    store: &Store,
    id: BoxId,
    box_asleep: bool,
) -> Result<Option<HeldMachineView>, CommandError> {
    let place = store.wait_place(id)?;
    let Some(h) = store.claimed_machine(id)? else {
        return Ok(place.map(|p| HeldMachineView {
            name: p.name,
            kind: None,
            vm_provider: None,
            slot: 0,
            ports: p.ports,
            state: MachineHoldState::Waiting,
            position: Some(p.position),
        }));
    };
    let state = match (h.state, box_asleep) {
        (MachineHoldState::Asleep, false) => MachineHoldState::Waiting,
        (state, _) => state,
    };
    Ok(Some(HeldMachineView {
        name: h.machine.name,
        kind: Some(h.machine.kind.name()),
        vm_provider: h.machine.kind.provider().map(str::to_owned),
        slot: h.slot,
        ports: h.ports,
        position: place
            .filter(|_| state == MachineHoldState::Waiting)
            .map(|p| p.position),
        state,
    }))
}

/// Run `show`.
pub(crate) fn run(cx: &Cx, req: &ShowReq) -> Result<ShowView, CommandError> {
    reconcile::run(cx);
    let store = cx.store()?;
    let now = cx.clock().now();
    let Some(target) = &req.target else {
        return list(cx, store, now);
    };
    let row = target::resolve(store, target, Rule::Latest)?
        .ok_or_else(|| CommandError::new(ErrorKind::Failed, format!("no Box {target}")))?;
    let in_flight = store.in_flight_run(row.id)?;
    let reading = activity::read(cx, store, &row, now, activity::PROBE_DEADLINE)?;
    let (agent, pane) = (agent_view(&reading), pane_view(&reading));
    let mut detail = box_view_with_run(&row, in_flight.as_ref(), cx.config());
    let asleep = row
        .boxed
        .as_ref()
        .is_some_and(|b| b.state == BoxState::Asleep);
    (detail.agent, detail.held_machine) = (agent, held_machine(store, row.id, asleep)?);
    detail.attention = attention(store, row.id)?;
    detail.said = if req.agent {
        said_of(store, row.id)?
    } else {
        None
    };
    hide_unsupported_sleep(&mut detail, cx);
    let events = match req.events {
        Some(n) => {
            let last = usize::try_from(n).unwrap_or(usize::MAX);
            let rows = store.events(row.id, last)?;
            Some(rows.iter().map(event_view).collect::<Vec<EventView>>())
        }
        None => None,
    };
    let artifacts = if req.artifacts {
        Some(artifacts_of(store, row.id)?)
    } else {
        None
    };
    let reports = if req.reports {
        Some(reports_of(store, row.id)?)
    } else {
        None
    };
    Ok(ShowView {
        boxes: None,
        boxed: Some(detail),
        events,
        pane: pane.filter(|_| req.pane),
        reports,
        artifacts,
    })
}

/// Every open Box the viewer may act on.
fn list(cx: &Cx, store: &Store, now: Ts) -> Result<ShowView, CommandError> {
    let mut boxes: Vec<BoxView> = Vec::new();
    let mut sleep_caps = HashMap::new();
    for mut row in store.open_boxes()? {
        if !cx.viewer().may_act_on(row.user) {
            continue;
        }
        let reading = activity::read(cx, store, &row, now, activity::PROBE_DEADLINE)?;
        activity::settle(cx, store, &row, &reading)?;
        autosleep::run(cx, store, row.id, reading.blocked());
        row = store.box_row(row.id)?.unwrap_or(row);
        let in_flight = store.in_flight_run(row.id)?;
        let mut view = box_view_with_run(&row, in_flight.as_ref(), cx.config());
        view.agent = agent_view(&reading);
        view.attention = attention(store, row.id)?;
        hide_unsupported_sleep_cached(&mut view, cx, &mut sleep_caps);
        boxes.push(view);
    }
    Ok(ShowView {
        boxes: Some(boxes),
        boxed: None,
        events: None,
        pane: None,
        reports: None,
        artifacts: None,
    })
}

#[cfg(test)]
mod tests;
