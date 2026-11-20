//! Settling the started runs of a workflow: finished, refused or expired.

use moor_core::decide::workflow::{
    waited_millis, workflow_expired, workflow_start_overdue, workflow_unanswered, RunBoxFacts,
    START_LIMIT,
};
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::deaths;
use moor_core::name::BoxName;
use moor_core::request::{CloseReq, EndSessionReq};
use moor_core::seam::clock::Ts;
use moor_core::view::progress::{failure, Happened};
use moor_core::view::LaunchFailure;
use moor_core::workflow::{
    box_name, HistoryEntry, RunId, RunOutcome, StepId, Workflow, WorkflowRun,
};
use moor_core::BoxId;

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
        death_capped: row.lifecycle == BoxLifecycle::Asleep && death_capped(store, row.id)?,
    })
}

/// Whether Box `id` died and moor did not resume it, and no wake has come since. Events
/// are numbered in the order they were written, so ids tell which came last.
fn death_capped(store: &Store, id: BoxId) -> Result<bool, StoreError> {
    let Some(capped) = store.newest_event_of(id, "death_capped")? else {
        return Ok(false);
    };
    let woke = store.newest_event_of(id, "woke")?;
    Ok(woke.is_none_or(|w| w.id < capped.id))
}

/// Fails the run at `at`, which works in Box `box_id` that died and was not resumed, with the
/// sentence its `death_capped` event told the owner. Nothing is sent to the Box: neither the
/// step's `fail` edge nor another try could run without waking it.
pub(super) fn fail_death_capped(
    store: &Store,
    w: &Workflow,
    run: &WorkflowRun,
    (box_id, at, try_n): (BoxId, &StepId, u8),
    now: Ts,
) -> Result<(), StoreError> {
    let said = store
        .newest_event_of(box_id, "death_capped")?
        .and_then(|e| Some(deaths::sentence(e.payload.get("reason")?.as_str()?)))
        .unwrap_or_else(|| "its machine stopped, so moor did not resume it".to_owned());
    let key = (w.id, &run.repo, run.due_at);
    let outcome = RunOutcome::Failed {
        at: at.clone(),
        reason: said,
    };
    store.end_workflow_run(key, (at, try_n), (box_id, outcome), (&[], now))?;
    Ok(())
}

/// Why no agent will run in `row`, naming the Box and its recorded failure: the launch failure,
/// else what its `setup_failed` event says, else the reason on its state.
pub(super) fn broken_detail(store: &Store, row: &BoxRow) -> String {
    let why = row
        .launch_failure
        .as_ref()
        .map(LaunchFailure::text)
        .or_else(|| setup_failure(store, row))
        .or_else(|| row.state_reason.clone())
        .unwrap_or_else(|| "no reason recorded".to_owned());
    format!(
        "Box {} is {} and no agent can run until it is retried: {why}",
        row.name.as_str(),
        row.lifecycle.as_str()
    )
}

/// The failed setup of `row` as its newest `setup_failed` event tells it.
fn setup_failure(store: &Store, row: &BoxRow) -> Option<String> {
    if row.lifecycle != BoxLifecycle::SetupFailed {
        return None;
    }
    let e = store.newest_event_of(row.id, "setup_failed").ok()??;
    failure(&[Happened {
        kind: e.kind,
        payload: e.payload,
        ts: String::new(),
    }])
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
    let waited = |run: RunId, from| {
        store
            .run_entries(run)
            .map_or(0, |l| waited_millis(&l, from))
    };
    for u in workflow_unanswered(&open, now) {
        let key = (w.id, &u.repo, u.due_at);
        let lines = [HistoryEntry::Unanswered { step: u.at.clone() }];
        let outcome = RunOutcome::Declined { at: u.at.clone() };
        store.end_waiting_run(key, (&u.at, u.try_n), (u.box_id, outcome), (&lines, now))?;
    }
    for e in workflow_expired(w, &open, &agent, &waited, now) {
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
    let waited = |run: RunId, from| {
        store
            .run_entries(run)
            .map_or(0, |l| waited_millis(&l, from))
    };
    for b in workflow_start_overdue(&open, (&agent, &waited), now) {
        let key = (w.id, &b.repo, b.due_at);
        let planned = match b.box_id {
            Some(id) => store.box_row(id)?.map(|r| r.name),
            None => box_name(w.id, w.name.as_str(), b.due_at, &b.cause).ok(),
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
                        "workflow {}: could not close the late Box of its run: {why}",
                        w.name
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
