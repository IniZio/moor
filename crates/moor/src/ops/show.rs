//! `show`: reconcile, then report tasks, one task's detail and its events.
//! Each task's agent is read once per command; a named task reads only its own.

use std::collections::HashMap;

use moor_core::view::{EventView, PaneView, ShowView, TaskView};
use moor_core::{ErrorKind, ShowReq, TaskId};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::view::{
    agent_view, event_view, hide_unsupported_sleep, hide_unsupported_sleep_cached,
    task_view_with_run,
};
use crate::ops::common::{activity, autosleep, reconcile};
use crate::store::Store;

/// Lines of the pane kept as the tail.
const PANE_LINES: usize = 40;

/// Stand-in for a proxy nonce in pane text.
const REDACTED: &str = "<nonce>";

/// Length of a base64url-encoded 32-byte nonce.
const NONCE_LEN: usize = 43;

/// `text` with the task's proxy nonce, and any `moor:<43 base64url chars>@` userinfo,
/// replaced by `<nonce>`. The proxy URL in the agent's environment carries the nonce.
fn redact_nonce(text: &str, nonce: Option<&[u8]>) -> String {
    const USER: &str = "moor:";
    let known = nonce
        .and_then(|n| std::str::from_utf8(n).ok())
        .filter(|n| !n.is_empty());
    let text = known.map_or_else(|| text.to_owned(), |n| text.replace(n, REDACTED));
    let mut out = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while let Some(at) = rest.find(USER) {
        let (head, tail) = rest.split_at(at + USER.len());
        out.push_str(head);
        let seg = tail.as_bytes();
        let is_nonce = seg.get(NONCE_LEN) == Some(&b'@')
            && seg
                .iter()
                .take(NONCE_LEN)
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'));
        if is_nonce {
            out.push_str(REDACTED);
            // The first 43 bytes are ASCII, so the split is on a char boundary.
            rest = tail.split_at(NONCE_LEN).1;
        } else {
            rest = tail;
        }
    }
    out.push_str(rest);
    out
}

/// The pane section of a reading: its last lines and liveness; none without an observation.
/// The task's proxy nonce is redacted from the tail.
fn pane_view(
    reading: &activity::Reading,
    store: &Store,
    task: TaskId,
) -> Result<Option<PaneView>, CommandError> {
    let Some(text) = reading.pane_text.as_deref() else {
        return Ok(None);
    };
    let row = store.proxy(task)?;
    let skip = text.lines().count().saturating_sub(PANE_LINES);
    let tail = text.lines().skip(skip).collect::<Vec<_>>().join("\n");
    let tail = redact_nonce(&tail, row.as_ref().map(|r| r.nonce.expose()));
    Ok(Some(PaneView {
        tail,
        status: reading.pane.as_str().to_owned(),
    }))
}

/// Run `show`.
pub(crate) fn run(cx: &Cx, req: &ShowReq) -> Result<ShowView, CommandError> {
    reconcile::run(cx);
    let store = cx.store()?;
    let now = cx.clock().now();
    let Some(name) = &req.task else {
        let mut tasks: Vec<TaskView> = Vec::new();
        let mut sleep_caps = HashMap::new();
        for mut row in store.open_tasks()? {
            let reading = activity::read(cx, store, &row, now)?;
            activity::settle(cx, store, &row, &reading)?;
            autosleep::run(cx, store, row.id);
            row = store.task(row.id)?.unwrap_or(row);
            let in_flight = store.in_flight_run(row.id, &|pid| cx.proc().alive(pid))?;
            let mut view = task_view_with_run(&row, in_flight.as_ref(), cx.config());
            view.agent = agent_view(&reading);
            hide_unsupported_sleep_cached(&mut view, cx, &mut sleep_caps);
            tasks.push(view);
        }
        return Ok(ShowView {
            tasks: Some(tasks),
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
    let reading = activity::read(cx, store, &row, now)?;
    let (agent, pane) = (agent_view(&reading), pane_view(&reading, store, row.id)?);
    let mut detail = task_view_with_run(&row, in_flight.as_ref(), cx.config());
    detail.agent = agent;
    hide_unsupported_sleep(&mut detail, cx);
    let events = match req.events {
        Some(n) => {
            let last = usize::try_from(n).unwrap_or(usize::MAX);
            let rows = store.events(row.id, last)?;
            Some(rows.iter().map(event_view).collect::<Vec<EventView>>())
        }
        None => None,
    };
    Ok(ShowView {
        tasks: None,
        task: Some(detail),
        events,
        pane: pane.filter(|_| req.pane),
        reports: None,
    })
}

#[cfg(test)]
mod tests;
