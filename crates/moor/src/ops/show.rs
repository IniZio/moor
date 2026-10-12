//! `show`: reconcile, then report tasks, one task's detail and its events.

use moor_core::view::{EventView, ShowView, TaskView};
use moor_core::{ErrorKind, ShowReq};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::reconcile;
use crate::ops::common::view::{event_view, task_view_with_run};

/// Run `show`.
pub(crate) fn run(cx: &Cx, req: &ShowReq) -> Result<ShowView, CommandError> {
    reconcile::run(cx);
    let store = cx.store()?;
    let mut tasks: Vec<TaskView> = Vec::new();
    for row in store.all_tasks()? {
        let in_flight = store.in_flight_run(row.id, &|pid| cx.proc().alive(pid))?;
        tasks.push(task_view_with_run(&row, in_flight.as_ref()));
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
        task: Some(task_view_with_run(&row, in_flight.as_ref())),
        events,
        pane: None,
        reports: None,
    })
}

#[cfg(test)]
mod tests;
