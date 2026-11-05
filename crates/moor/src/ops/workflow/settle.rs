//! Settling the started runs of a workflow: finished, refused or expired.

use moor_core::decide::workflow::{run_verdict, workflow_expired, RunBoxFacts, RunVerdict};
use moor_core::request::EndSessionReq;
use moor_core::seam::clock::Ts;
use moor_core::view::LaunchFailure;
use moor_core::workflow::{RunEnd, Workflow, WorkflowOutcome};

use crate::cx::Cx;
use crate::ops::end_session;
use crate::store::{BoxRow, Store, StoreError};

/// What the tick reads of Box `row` to tell where its run stands.
pub(super) const fn box_facts(row: &BoxRow) -> RunBoxFacts {
    RunBoxFacts {
        lifecycle: row.lifecycle,
        session: row.session,
        idle_since: row.idle_since,
        launch_failed: row.launch_failure.is_some(),
    }
}

/// Why no agent will run in `row`, naming the Box and its recorded failure: the launch failure,
/// else the line of its latest `setup_failed` event, else the reason on its state.
pub(super) fn broken_detail(store: &Store, row: &BoxRow) -> Result<String, StoreError> {
    let cause = match store.last_event_of(row.id, "setup_failed")? {
        Some(e) => e
            .payload
            .get("line")
            .and_then(|c| c.as_str())
            .map(str::to_owned),
        None => None,
    };
    let why = row
        .launch_failure
        .as_ref()
        .map(LaunchFailure::text)
        .or(cause)
        .or_else(|| row.state_reason.clone())
        .unwrap_or_else(|| "no reason recorded".to_owned());
    Ok(format!(
        "Box {} is {} and no agent can run until it is retried: {why}",
        row.name.as_str(),
        row.lifecycle.as_str()
    ))
}

/// Settles each started run of `w` whose Box is over: finished, or refused when no agent can
/// run in it until a person acts. The Box stays open for inspection and nothing retries it.
pub(super) fn finish_runs(store: &Store, w: &Workflow, now: Ts) -> Result<(), StoreError> {
    for run in store.open_workflow_runs(w.id)? {
        let WorkflowOutcome::Started { box_id } = run.outcome else {
            continue;
        };
        let row = store.box_row(box_id)?;
        let key = (w.id, &run.repo, run.due_at);
        match run_verdict(row.as_ref().map(box_facts).as_ref(), now) {
            RunVerdict::Running => {}
            RunVerdict::Finished => {
                store.end_workflow_run(key, RunEnd::Finished, now)?;
            }
            RunVerdict::Broken => {
                if let Some(row) = &row {
                    store.refuse_started_run(key, &broken_detail(store, row)?)?;
                }
            }
        }
    }
    Ok(())
}

/// Ends the agent session of each started run past its timeout, keeps the Box, and records the
/// run as timed out whether or not a session was live to end.
pub(super) fn expire_runs(
    cx: &Cx,
    store: &Store,
    w: &Workflow,
    now: Ts,
    out: &mut Vec<String>,
) -> Result<(), StoreError> {
    let open = store.open_workflow_runs(w.id)?;
    for e in workflow_expired(w, &open, now) {
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
        store.end_workflow_run((w.id, &e.repo, e.due_at), RunEnd::TimedOut, now)?;
    }
    Ok(())
}
