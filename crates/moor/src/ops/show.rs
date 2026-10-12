//! `show`: reconcile, then report tasks, one task's detail and its events.

use moor_core::view::{EventView, ShowView, TaskView};
use moor_core::{ErrorKind, ShowReq};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::reconcile;
use crate::ops::common::view::{event_view, task_view};

/// Run `show`.
pub(crate) fn run(cx: &Cx, req: &ShowReq) -> Result<ShowView, CommandError> {
    reconcile::run(cx);
    let store = cx.store()?;
    let tasks: Vec<TaskView> = store.all_tasks()?.iter().map(task_view).collect();
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
        task: Some(task_view(&row)),
        events,
        pane: None,
        reports: None,
    })
}

#[cfg(test)]
mod tests;
