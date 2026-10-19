//! `wait-events`: poll the event log until any or all of several Boxes emit matching events.
//!
//! Never takes a Box lock: it settles each Box through the store, which writes the
//! `turn_done` and `session_ended` events it waits for.

use std::collections::BTreeSet;
use std::time::Duration;

use moor_core::controller::ServerFrame;
use moor_core::decide::wait_events::{wait_events_verdict, WaitEventsVerdict};
use moor_core::domain::boxes::BoxState;
use moor_core::{BoxId, BoxName, ErrorKind, WaitEventsReq, Warning};
use serde::Serialize;

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::{activity, reconcile};
use crate::store::Store;

/// Longest pause between polls.
const POLL: Duration = Duration::from_millis(250);

/// How long `wait-events` polls when the request sets no timeout.
const DEFAULT_TIMEOUT: Duration = Duration::from_mins(30);

/// Events read from the log per query.
const BATCH: usize = 256;

/// The `wait-events` result.
#[derive(Debug, Serialize)]
pub(crate) struct WaitEventsValue {
    /// The matching events seen, oldest first.
    pub(crate) events: Vec<ServerFrame>,
    /// The last event id scanned; pass it as the next `since`.
    pub(crate) cursor: i64,
    /// Whether the timeout ended the wait before the condition held.
    pub(crate) timed_out: bool,
    /// Milliseconds spent waiting.
    pub(crate) waited_ms: u64,
}

/// Run `wait-events`.
pub(crate) fn run(cx: &Cx, req: &WaitEventsReq) -> Result<WaitEventsValue, CommandError> {
    let clock = cx.clock();
    let started = clock.instant();
    reconcile::run(cx);
    let store = cx.store()?;
    let watched = resolve(store, &req.boxes)?;
    let timeout = req
        .timeout_secs
        .map_or(DEFAULT_TIMEOUT, Duration::from_secs);
    let mut cursor = match req.since {
        Some(since) => since,
        None => store.last_event_id()?,
    };
    let mut events = Vec::new();
    let mut matched: Vec<BoxName> = Vec::new();
    let mut warned: BTreeSet<BoxId> = BTreeSet::new();
    loop {
        let elapsed = clock.instant().saturating_duration_since(started);
        for (id, _) in &watched {
            settle(cx, store, *id, &mut warned)?;
        }
        loop {
            let batch = store.events_after(cursor, BATCH)?;
            let short = batch.len() < BATCH;
            for event in batch {
                cursor = event.row.id;
                let name = watched.iter().find(|(id, _)| *id == event.row.box_id);
                let kind_ok = req
                    .kinds
                    .as_ref()
                    .is_none_or(|kinds| kinds.contains(&event.row.kind));
                if let (Some((_, name)), true) = (name, kind_ok) {
                    if !matched.contains(name) {
                        matched.push((*name).clone());
                    }
                    events.push(event.into_frame());
                }
            }
            if short {
                break;
            }
        }
        let verdict = wait_events_verdict(req.mode, &req.boxes, &matched, elapsed, timeout);
        if verdict == WaitEventsVerdict::Keep {
            clock.sleep(POLL.min(timeout.saturating_sub(elapsed)));
            continue;
        }
        return Ok(WaitEventsValue {
            events,
            cursor,
            timed_out: verdict == WaitEventsVerdict::TimedOut,
            waited_ms: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
        });
    }
}

/// The latest Task of each name; an empty, unknown or closed list is refused.
fn resolve<'a>(
    store: &Store,
    names: &'a [BoxName],
) -> Result<Vec<(BoxId, &'a BoxName)>, CommandError> {
    if names.is_empty() {
        return Err(CommandError::new(
            ErrorKind::Failed,
            "wait-events needs at least one Box",
        ));
    }
    let mut watched = Vec::new();
    for name in names {
        let row = store.latest_box_by_name(name)?.ok_or_else(|| {
            CommandError::new(ErrorKind::Failed, format!("no task {}", name.as_str()))
        })?;
        if !row.lifecycle.is_live() {
            return Err(CommandError::new(
                ErrorKind::Failed,
                format!("task {} is closed", name.as_str()),
            ));
        }
        watched.push((row.id, name));
    }
    Ok(watched)
}

/// Settle one watched Box when it is ready and its setup has not failed; a failure warns once per Box.
fn settle(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    warned: &mut BTreeSet<BoxId>,
) -> Result<(), CommandError> {
    let Some(row) = store.box_row(id)? else {
        return Ok(());
    };
    let ready = row.boxed.as_ref().map(|b| b.state) == Some(BoxState::Ready);
    if !ready || row.setup_failed_at.is_some() {
        return Ok(());
    }
    let settled = activity::read(cx, store, &row, cx.clock().now())
        .and_then(|reading| activity::settle(cx, store, &row, &reading));
    if let Err(e) = settled {
        if warned.insert(id) {
            cx.warn(Warning {
                code: "settle_failed".to_owned(),
                message: format!("could not settle task {}: {}", id.get(), e.error),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
