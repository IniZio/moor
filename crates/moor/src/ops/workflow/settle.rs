//! Settling the started runs of a workflow: finished, refused or expired.

use moor_core::decide::workflow::{
    workflow_expired, workflow_start_overdue, RunBoxFacts, START_LIMIT,
};
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::name::BoxName;
use moor_core::request::{CloseReq, EndSessionReq};
use moor_core::seam::clock::Ts;
use moor_core::view::LaunchFailure;
use moor_core::workflow::{box_name, RunOutcome, Workflow};

use crate::cx::Cx;
use crate::ops::{close, end_session};
use crate::store::{BoxRow, Store, StoreError};

/// What the tick reads of Box `row` to tell where its run stands.
///
/// A sleep clears the Box's `idle_since`, so an asleep Box whose agent went quiet before the
/// sleep reads as idle since its last `idled` event. One put to sleep while its agent worked
/// has no such time: the cut turn did not finish its step.
pub(super) fn box_facts(store: &Store, row: &BoxRow) -> Result<RunBoxFacts, StoreError> {
    let idle_since = if row.lifecycle == BoxLifecycle::Asleep {
        match row.parked_at_work {
            Some(_) => None,
            None => store.newest_event_of(row.id, "idled")?.map(|e| e.ts),
        }
    } else {
        row.idle_since
    };
    Ok(RunBoxFacts {
        lifecycle: row.lifecycle,
        session: row.session,
        idle_since,
        launch_failed: row.launch_failure.is_some(),
    })
}

/// Why no agent will run in `row`, naming the Box and its recorded failure: the launch failure,
/// else the reason on its state, which a failed setup sets to the line that failed.
pub(super) fn broken_detail(row: &BoxRow) -> String {
    let why = row
        .launch_failure
        .as_ref()
        .map(LaunchFailure::text)
        .or_else(|| row.state_reason.clone())
        .unwrap_or_else(|| "no reason recorded".to_owned());
    format!(
        "Box {} is {} and no agent can run until it is retried: {why}",
        row.name.as_str(),
        row.lifecycle.as_str()
    )
}

/// Ends the agent session of each started run whose agent has worked past its timeout, keeps the Box, and records the
/// run as timed out whether or not a session was live to end.
pub(super) fn expire_runs(
    cx: &Cx,
    store: &Store,
    w: &Workflow,
    now: Ts,
    out: &mut Vec<String>,
) -> Result<(), StoreError> {
    let open = store.open_workflow_runs(w.id)?;
    let agent = |id| store.first_launch_at(id).ok().flatten();
    for e in workflow_expired(w, &open, &agent, now) {
        if let Some(row) = store.box_row(e.box_id)? {
            let req = EndSessionReq {
                box_name: row.name.as_str().to_owned(),
            };
            if let Err(err) = cx.for_box(row.user, || end_session::run(cx, &req)) {
                out.push(format!(
                    "workflow {}: {}: {}",
                    w.name,
                    row.name.as_str(),
                    err.error
                ));
            }
        }
        let key = (w.id, &e.repo, e.due_at);
        let outcome = RunOutcome::TimedOut { at: e.at.clone() };
        store.end_workflow_run(key, (&e.at, e.try_n), (e.box_id, outcome), (&[], now))?;
    }
    Ok(())
}

/// Closes the open Box called `name`, forcing it, which also stops a start still setting it up.
/// False when there is no open Box of that name.
pub(crate) fn close_open_box(cx: &Cx, store: &Store, name: &BoxName) -> Result<bool, String> {
    let Some(row) = store.open_box_by_name(name).map_err(|e| e.to_string())? else {
        return Ok(false);
    };
    let req = CloseReq {
        target: row.name.clone().into(),
        force: true,
    };
    cx.for_box(row.user, || close::run(cx, &req))
        .map(|_| true)
        .map_err(|e| e.error.to_string())
}

/// Refuses each run of `w` whose agent has not started [`START_LIMIT`] after the run was
/// claimed, and closes its Box so the agent can never start later. A claimed run is refused
/// first, which settles it before its own start can, and its Box is closed after; a started
/// run whose Box is queued or stuck keeps its place until the Box is closed.
pub(super) fn enforce_start_limit(
    cx: &Cx,
    store: &Store,
    w: &Workflow,
    now: Ts,
    out: &mut Vec<String>,
) -> Result<(), StoreError> {
    let open = store.open_workflow_runs(w.id)?;
    let agent = |id| store.first_launch_at(id).ok().flatten();
    for b in workflow_start_overdue(&open, &agent, now) {
        let key = (w.id, &b.repo, b.due_at);
        let planned = match b.box_id {
            Some(id) => store.box_row(id)?.map(|r| r.name),
            None => box_name(w.id, w.name.as_str(), &b.repo, b.due_at).ok(),
        };
        let detail = format!(
            "the run's agent did not start within {} minutes of the firing, so the run was \
             stopped and its Box closed",
            START_LIMIT.as_secs() / 60
        );
        let close =
            |out: &mut Vec<String>| match planned.as_ref().map(|n| close_open_box(cx, store, n)) {
                Some(Err(why)) => {
                    out.push(format!(
                        "workflow {}: could not close the late Box of {}: {why}",
                        w.name, b.repo
                    ));
                    false
                }
                _ => true,
            };
        if b.claimed_only {
            if store.refuse_run(key, (&detail, true), now)? {
                close(out);
            }
        } else if close(out) {
            store.refuse_run(key, (&detail, false), now)?;
        }
    }
    Ok(())
}
