//! `workflow-run-now`: the owner fires a workflow once, now.
//!
//! The run goes through the same preparation and start a cron firing uses. The claim is a manual
//! one: it is recorded only when the workflow has no open run, which the store checks in the write
//! that inserts the row. A refused run records nothing. A recorded claim's Box starts on a thread
//! of its own, as a cron firing's does, so the answer never waits on a Box's setup; a start that
//! is refused then settles the run as refused, which `list_workflow_runs` shows.

use std::thread;

use moor_core::user::Viewer;
use moor_core::view::workflow::RunNowValue;
use moor_core::workflow::{
    box_name, NewRun, RunState, RunTrigger, Workflow, WorkflowName, WorkflowRun,
};
use moor_core::ErrorKind;

use super::{prepare, run_start, Prepared, StartJob};
use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::{ManualClaim, Store};

/// A refusal the caller can act on.
fn refused(why: impl std::fmt::Display) -> CommandError {
    CommandError::new(ErrorKind::Refused, why.to_string())
}

/// A refusal for input the caller can correct.
fn usage(why: impl std::fmt::Display) -> CommandError {
    CommandError::new(ErrorKind::Usage, why.to_string())
}

/// Fires `user`'s workflow `name` once. `expected_revision` refuses the call when the workflow
/// changed after the caller read it.
pub(crate) fn run(
    cx: &Cx,
    store: &Store,
    user: moor_core::user::UserId,
    args: (&str, Option<u64>),
) -> Result<RunNowValue, CommandError> {
    run_with(cx, store, user, args, &|job| spawn_start(cx, job))
}

/// Like [`run`], handing a recorded claim to `start`.
pub(super) fn run_with(
    cx: &Cx,
    store: &Store,
    user: moor_core::user::UserId,
    (name, expected_revision): (&str, Option<u64>),
    start: &dyn Fn(StartJob) -> Result<(), CommandError>,
) -> Result<RunNowValue, CommandError> {
    let name = WorkflowName::parse(name).map_err(usage)?;
    let Some(w) = store.workflow(user, &name)? else {
        return Err(refused(format!("no workflow named {name}")));
    };
    if let Some(wanted) = expected_revision.filter(|r| *r != w.revision) {
        return Err(refused(format!(
            "workflow {name} is at revision {}, not revision {wanted}: it changed after you read it",
            w.revision
        ))
        .with_hint("read it again with list_workflows and run it with that revision"));
    }
    fire(cx, store, &w, start)
}

/// A run that was refused, and why.
fn declined(w: &Workflow, why: String) -> RunNowValue {
    RunNowValue {
        workflow: w.name.to_string(),
        status: "refused".to_owned(),
        box_name: None,
        reason: Some(why),
    }
}

/// Claims a manual run of `w` and hands it to `start`. A claim the store refuses records
/// nothing; a start that fails after the claim leaves a refused run, as a cron firing does.
fn fire(
    cx: &Cx,
    store: &Store,
    w: &Workflow,
    start: &dyn Fn(StartJob) -> Result<(), CommandError>,
) -> Result<RunNowValue, CommandError> {
    let now = cx.clock().now();
    let Prepared { job, frozen } = match prepare(cx, store, w, (now, RunTrigger::Manual)) {
        Ok(prepared) => prepared,
        Err(why) => return Ok(declined(w, why)),
    };
    let claim = NewRun {
        workflow: w.id,
        repo: job.repo.clone(),
        due_at: now,
        state: RunState::Queued,
        recorded_at: now,
        plan: frozen,
        trigger: RunTrigger::Manual,
    };
    match store.claim_manual_run(&claim)? {
        ManualClaim::Open(open) => {
            let box_name = open_box(store, w, &open)?;
            let phase = open.state.phase().as_str();
            Ok(declined(
                w,
                format!(
                    "a run of {} is still open ({phase}, Box {box_name})",
                    w.name
                ),
            ))
        }
        ManualClaim::Taken => Ok(declined(
            w,
            format!("a run of {} was fired at this very moment already", w.name),
        )),
        ManualClaim::Recorded => {
            let name = job.name.as_str().to_owned();
            start(job)?;
            Ok(RunNowValue {
                workflow: w.name.to_string(),
                status: "started".to_owned(),
                box_name: Some(name),
                reason: None,
            })
        }
    }
}

/// Starts `job`'s Box on a thread of its own, as its owner, which settles the claim.
fn spawn_start(cx: &Cx, job: StartJob) -> Result<(), CommandError> {
    let owner = cx
        .detached()?
        .with_requester(job.identity.clone())
        .with_viewer(Viewer::User(job.owner));
    thread::spawn(move || {
        crate::cx::owners::checked(|| {
            run_start(&owner, &job);
        });
    });
    Ok(())
}

/// The name of the Box `open` works in, or will work in while it is being started.
fn open_box(store: &Store, w: &Workflow, open: &WorkflowRun) -> Result<String, CommandError> {
    let existing = match open.state.box_id() {
        Some(id) => store.box_row(id)?.map(|r| r.name),
        None => None,
    };
    Ok(existing
        .or_else(|| box_name(w.id, w.name.as_str(), open.due_at, open.trigger).ok())
        .map_or_else(|| "unknown".to_owned(), |n| n.as_str().to_owned()))
}
