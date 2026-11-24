//! `show`: reconcile, then report Boxes, one Box's detail and its events.
//! Each Box's agent is read once per command; a named Box reads only its own.

use moor_core::domain::hands::HandsState;
use moor_core::progress::transcript::Transcript;
use moor_core::progress::TimelineEntry;
use moor_core::seam::clock::Ts;
use moor_core::view::gate::GateView;
use moor_core::view::{
    ApprovalView, ArtifactView, AttentionView, BoxView, EventView, HeldMachineView,
    MachineHoldState, PaneView, ReportView, SaidView, ShowView,
};
use moor_core::workflow::{RunState, StepKind};
use moor_core::{progress, BoxId, ErrorKind, ShowReq};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::artifacts::artifact_view;
use crate::ops::authz;
use crate::ops::common::target::{self, Rule};
use crate::ops::common::view::{agent_view, box_view_with_run, event_view, rfc3339};
use crate::ops::common::{activity, autosleep, reconcile, transcript};
use crate::store::{EventRow, Store};

/// Events a Box's state and line are derived from.
const PROGRESS_EVENTS: usize = 200;

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

/// The Box's gates and where it stands against them; none for a Box without gates.
fn gate_of(store: &Store, box_id: BoxId) -> Result<Option<GateView>, CommandError> {
    let Some(gates) = store.box_gates(box_id)? else {
        return Ok(None);
    };
    let view = GateView::of(&gates, &store.gate_lines(box_id)?);
    Ok(Some(view.ended(store.run_end_of_box(box_id)?)))
}

/// The approve step of the workflow run that waits in the Box, when one does.
fn approval_of(store: &Store, box_id: BoxId) -> Result<Option<ApprovalView>, CommandError> {
    let Some((workflow, run)) = store.running_run_of_box(box_id)? else {
        return Ok(None);
    };
    let RunState::Waiting { at, .. } = &run.state else {
        return Ok(None);
    };
    let Some(w) = store.workflow_by_id(workflow)? else {
        return Ok(None);
    };
    let plan = store.run_plan(run.id, &w.prompt)?;
    let Some((title, question)) = plan.step(at).and_then(|s| match &s.kind {
        StepKind::Approve { question, .. } => Some((s.title.clone(), question.clone())),
        _ => None,
    }) else {
        return Ok(None);
    };
    Ok(Some(ApprovalView {
        workflow: w.name.to_string(),
        run: run.id.get(),
        step: title,
        question,
    }))
}

/// What Box `id` is tied to.
fn ties_of_one(store: &Store, id: BoxId) -> Result<moor_core::view::BoxTies, CommandError> {
    Ok(store.ties_of(&[id])?.remove(&id).unwrap_or_default())
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
pub(crate) fn said_of(store: &Store, box_id: BoxId) -> Result<Option<SaidView>, CommandError> {
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
        if let Some(p) = place {
            return Ok(Some(HeldMachineView {
                name: p.name,
                slot: 0,
                ports: p.ports,
                state: MachineHoldState::Waiting,
                position: Some(p.position),
            }));
        }
        return node_queue_place(store, id);
    };
    let state = match (h.state, box_asleep) {
        (MachineHoldState::Asleep, false) => MachineHoldState::Waiting,
        (state, _) => state,
    };
    Ok(Some(HeldMachineView {
        name: h.machine.name,
        slot: h.slot,
        ports: h.ports,
        position: place
            .filter(|_| state == MachineHoldState::Waiting)
            .map(|p| p.position),
        state,
    }))
}

/// Where Box `id` stands in the queue on its node, when it waits there.
fn node_queue_place(store: &Store, id: BoxId) -> Result<Option<HeldMachineView>, CommandError> {
    let Some(node) = store.box_spec(id)?.node else {
        return Ok(None);
    };
    let at = store.pending_on(&node)?.iter().position(|b| *b == id);
    Ok(at.map(|at| HeldMachineView {
        name: node.as_str().to_owned(),
        slot: 0,
        ports: Vec::new(),
        state: MachineHoldState::Waiting,
        position: Some(u32::try_from(at + 1).unwrap_or(u32::MAX)),
    }))
}

/// Run `show`.
pub(crate) fn run(cx: &Cx, req: &ShowReq) -> Result<ShowView, CommandError> {
    reconcile::run(cx);
    let store = cx.store()?;
    let now = cx.clock().now();
    let Some(target) = &req.target else {
        return list(cx, store, now, req.everyone);
    };
    let row = target::resolve(store, target, Rule::Latest)?
        .ok_or_else(|| CommandError::new(ErrorKind::Failed, format!("no Box {target}")))?;
    let in_flight = reconcile::run_in_flight(cx, store, row.id)?;
    let reading = activity::read(cx, store, &row, now, activity::PROBE_DEADLINE)?;
    let (agent, pane) = (agent_view(&reading), pane_view(&reading));
    let mut detail = box_view_with_run(&row, in_flight.as_ref(), cx.config());
    let asleep = row
        .hands
        .as_ref()
        .is_some_and(|b| b.state == HandsState::Asleep);
    (detail.agent, detail.held_machine) = (agent, held_machine(store, row.id, asleep)?);
    detail.attention = attention(store, row.id)?;
    detail.approval = approval_of(store, row.id)?;
    detail.gate = gate_of(store, row.id)?;
    detail.ties = ties_of_one(store, row.id)?;
    detail.owner = owner_of(cx, store, row.user)?;
    detail.said = if req.agent {
        said_of(store, row.id)?
    } else {
        None
    };
    let recent: Vec<EventView> = store
        .events(row.id, PROGRESS_EVENTS)?
        .iter()
        .map(event_view)
        .collect();
    detail.progress = Some(progress::derive(&detail, &recent, now.as_millis()));
    let rows = match req.events {
        Some(n) => Some(store.events(row.id, usize::try_from(n).unwrap_or(usize::MAX))?),
        None => None,
    };
    let events: Option<Vec<EventView>> = rows.as_ref().map(|r| r.iter().map(event_view).collect());
    let timeline = rows
        .as_deref()
        .map(|rows| timeline_of(cx, store, row.id, rows, req.transcript));
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
        timeline,
    })
}

/// Box `id`'s timeline over `rows`, with the lines of its saved transcript when `transcript`.
fn timeline_of(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    rows: &[EventRow],
    transcript: bool,
) -> Vec<TimelineEntry> {
    let said = transcript
        .then(|| transcript::lines::of(cx, store, id))
        .flatten();
    let (lines, saved_ms) = said.as_ref().map_or((&[][..], 0), |(l, at)| (&l[..], *at));
    let timed: Vec<(i64, EventView)> = rows
        .iter()
        .map(|r| (r.ts.as_millis(), event_view(r)))
        .collect();
    progress::timeline_with(&timed, Transcript { lines, saved_ms })
}

/// Who a Box that acts for `owner` acts for, when the viewer may not act on it: the user's email,
/// else the user's number.
fn owner_of(
    cx: &Cx,
    store: &Store,
    owner: Option<moor_core::user::UserId>,
) -> Result<Option<String>, CommandError> {
    let Some(user) = owner.filter(|_| !cx.viewer().may_act_on(owner)) else {
        return Ok(None);
    };
    let email = store.email_of(user)?;
    Ok(Some(
        email.unwrap_or_else(|| format!("user {}", user.get())),
    ))
}

/// Every open Box the viewer may act on, or with `everyone` may read.
fn list(cx: &Cx, store: &Store, now: Ts, everyone: bool) -> Result<ShowView, CommandError> {
    let mut boxes: Vec<BoxView> = Vec::new();
    let open: Vec<_> = store
        .open_boxes()?
        .into_iter()
        .filter(|row| !row.kind.is_idle_place())
        .filter(|row| authz::may_read(cx.viewer(), row.user, everyone))
        .collect();
    let ids: Vec<BoxId> = open.iter().map(|row| row.id).collect();
    let mut ties = store.ties_of(&ids)?;
    for mut row in open {
        let reading = activity::read(cx, store, &row, now, activity::PROBE_DEADLINE)?;
        activity::settle(cx, store, &row, &reading)?;
        autosleep::run_after_read(cx, store, row.id, &reading);
        row = store.box_row(row.id)?.unwrap_or(row);
        let in_flight = reconcile::run_in_flight(cx, store, row.id)?;
        let mut view = box_view_with_run(&row, in_flight.as_ref(), cx.config());
        view.agent = agent_view(&reading);
        view.attention = attention(store, row.id)?;
        view.approval = approval_of(store, row.id)?;
        view.gate = gate_of(store, row.id)?;
        view.ties = ties.remove(&row.id).unwrap_or_default();
        view.owner = owner_of(cx, store, row.user)?;
        let events = store.events(row.id, PROGRESS_EVENTS)?;
        let events: Vec<EventView> = events.iter().map(event_view).collect();
        view.progress = Some(progress::derive(&view, &events, now.as_millis()));
        boxes.push(view);
    }
    Ok(ShowView {
        boxes: Some(boxes),
        boxed: None,
        events: None,
        pane: None,
        reports: None,
        artifacts: None,
        timeline: None,
    })
}

#[cfg(test)]
mod tests;
