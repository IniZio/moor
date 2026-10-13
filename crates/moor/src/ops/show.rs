//! `show`: reconcile, then report tasks, one task's detail and its events.
//! Each task's agent is read once and the list and detail share that read.

use std::collections::HashMap;

use moor_core::view::{AgentView, EventView, ShowView, TaskView};
use moor_core::{ErrorKind, ShowReq, TaskId};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::view::{agent_view, event_view, task_view_with_run};
use crate::ops::common::{activity, reconcile};

/// Run `show`.
pub(crate) fn run(cx: &Cx, req: &ShowReq) -> Result<ShowView, CommandError> {
    reconcile::run(cx);
    let store = cx.store()?;
    let now = cx.clock().now();
    let mut tasks: Vec<TaskView> = Vec::new();
    let mut agents: HashMap<TaskId, Option<AgentView>> = HashMap::new();
    for row in store.all_tasks()? {
        let in_flight = store.in_flight_run(row.id, &|pid| cx.proc().alive(pid))?;
        let reading = activity::read(cx, store, &row, now)?;
        let agent = agent_view(&reading);
        let mut view = task_view_with_run(&row, in_flight.as_ref());
        view.agent.clone_from(&agent);
        agents.insert(row.id, agent);
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
    let agent = match agents.remove(&row.id) {
        Some(agent) => agent,
        None => agent_view(&activity::read(cx, store, &row, now)?),
    };
    let mut detail = task_view_with_run(&row, in_flight.as_ref());
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
        pane: None,
        reports: None,
    })
}

#[cfg(test)]
mod tests;
