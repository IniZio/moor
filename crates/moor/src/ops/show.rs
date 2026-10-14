//! `show`: reconcile, then report tasks, one task's detail and its events.
//! Each task's agent is read once and the list and detail share that read.

use std::collections::HashMap;

use moor_core::view::{AgentView, EventView, PaneView, ShowView, TaskView};
use moor_core::{ErrorKind, ShowReq, TaskId};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::view::{agent_view, event_view, task_view_with_run};
use crate::ops::common::{activity, autosleep, reconcile};

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

/// Run `show`.
pub(crate) fn run(cx: &Cx, req: &ShowReq) -> Result<ShowView, CommandError> {
    reconcile::run(cx);
    let store = cx.store()?;
    let now = cx.clock().now();
    let mut tasks: Vec<TaskView> = Vec::new();
    let mut agents: HashMap<TaskId, (Option<AgentView>, Option<PaneView>)> = HashMap::new();
    for mut row in store.all_tasks()? {
        let reading = activity::read(cx, store, &row, now)?;
        if req.task.is_none() {
            activity::settle(cx, store, &row, &reading)?;
            autosleep::run(cx, store, row.id);
            row = store.task(row.id)?.unwrap_or(row);
        }
        let in_flight = store.in_flight_run(row.id, &|pid| cx.proc().alive(pid))?;
        let agent = agent_view(&reading);
        let mut view = task_view_with_run(&row, in_flight.as_ref(), cx.config());
        view.agent.clone_from(&agent);
        agents.insert(row.id, (agent, pane_view(&reading)));
        tasks.push(view);
    }
    let Some(name) = &req.task else {
        return Ok(ShowView {
            tasks,
            task: None,
            events: None,
            pane: None,
            reports: None,
        });
    };
    let row = store.latest_task_by_name(name)?.ok_or_else(|| {
        CommandError::new(ErrorKind::Failed, format!("no task {}", name.as_str()))
    })?;
    let in_flight = store.in_flight_run(row.id, &|pid| cx.proc().alive(pid))?;
    let (agent, pane) = if let Some(found) = agents.remove(&row.id) {
        found
    } else {
        let reading = activity::read(cx, store, &row, now)?;
        (agent_view(&reading), pane_view(&reading))
    };
    let mut detail = task_view_with_run(&row, in_flight.as_ref(), cx.config());
    detail.agent = agent;
    let events = match req.events {
        Some(n) => {
            let last = usize::try_from(n).unwrap_or(usize::MAX);
            let rows = store.events(row.id, last)?;
            Some(rows.iter().map(event_view).collect::<Vec<EventView>>())
        }
        None => None,
    };
    Ok(ShowView {
        tasks,
        task: Some(detail),
        events,
        pane: pane.filter(|_| req.pane),
        reports: None,
    })
}

#[cfg(test)]
mod tests;
