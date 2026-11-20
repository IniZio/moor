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
    box_name, CommitSha, NewRun, PullRef, PullTarget, RunCause, RunState, Workflow, WorkflowName,
    WorkflowRun,
};
use moor_core::ErrorKind;

use super::cancel::stop_agent;
use super::{prepare, pull, run_start, Prepared, StartJob};
use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::{OpenClaim, Store};

/// A refusal the caller can act on.
fn refused(why: impl std::fmt::Display) -> CommandError {
    CommandError::new(ErrorKind::Refused, why.to_string())
}

/// A refusal for input the caller can correct.
fn usage(why: impl std::fmt::Display) -> CommandError {
    CommandError::new(ErrorKind::Usage, why.to_string())
}

/// Reads the head commit of a pull request.
pub(super) type ReadHead<'a> = &'a dyn Fn(&PullTarget) -> Result<CommitSha, CommandError>;

/// Hands a recorded claim to its start.
pub(super) type Start<'a> = &'a dyn Fn(StartJob) -> Result<(), CommandError>;

/// Fires `user`'s workflow `name` once, for pull request `pull` at its current head when one is
/// named. `expected_revision` refuses the call when the workflow changed after the caller read it.
pub(crate) fn run(
    cx: &Cx,
    store: &Store,
    user: moor_core::user::UserId,
    args: (&str, Option<u64>, Option<&PullTarget>),
) -> Result<RunNowValue, CommandError> {
    let read = |target: &PullTarget| pull::read_head(cx, target);
    run_with(cx, store, user, args, (&read, &|job| spawn_start(cx, job)))
}

/// Like [`run`], reading a pull request's head with `read` and handing a recorded claim to
/// `start`.
pub(super) fn run_with(
    cx: &Cx,
    store: &Store,
    user: moor_core::user::UserId,
    (name, expected_revision, target): (&str, Option<u64>, Option<&PullTarget>),
    (read, start): (ReadHead<'_>, Start<'_>),
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
    let pull = target.map(|t| pull_at_head(&w, t, read)).transpose()?;
    fire(cx, store, &w, &RunCause::Manual { pull }, start)
}

/// Pull request `target` at the head `read` gives, when it is on `w`'s first repo. The ref
/// names the repo as the workflow does; GitHub reads either spelling.
fn pull_at_head(
    w: &Workflow,
    target: &PullTarget,
    read: ReadHead<'_>,
) -> Result<PullRef, CommandError> {
    let first = w
        .repos
        .first()
        .ok_or_else(|| refused(format!("workflow {} has no repo", w.name)))?;
    let same = |a: &str, b: &str| a.eq_ignore_ascii_case(b);
    if !(same(&target.repo.owner, &first.owner) && same(&target.repo.name, &first.name)) {
        return Err(refused(format!(
            "pull request {target} is not on {first}, the first repo of workflow {}",
            w.name
        )));
    }
    let head_sha = read(target)?;
    Ok(PullRef {
        repo: first.clone(),
        number: target.number,
        head_sha,
    })
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

/// Claims a manual or pull request run of `w` for `cause` and hands it to `start`. A claim
/// the store refuses records nothing; a start that fails after the claim leaves a refused run,
/// as a cron firing does.
pub(super) fn fire(
    cx: &Cx,
    store: &Store,
    w: &Workflow,
    cause: &RunCause,
    start: Start<'_>,
) -> Result<RunNowValue, CommandError> {
    let now = cx.clock().now();
    let Prepared { job, frozen } = match prepare(cx, store, w, (now, cause)) {
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
        cause: cause.clone(),
    };
    match store.claim_open_run(&claim)? {
        OpenClaim::Open(open) => {
            let box_name = open_box(store, w, &open)?;
            let phase = open.state.phase().as_str();
            let of = open
                .cause
                .subject()
                .map_or_else(String::new, |p| format!(" for {}#{}", p.repo, p.number));
            Ok(declined(
                w,
                format!(
                    "a run of {}{of} is still open ({phase}, Box {box_name})",
                    w.name
                ),
            ))
        }
        OpenClaim::Held => Ok(declined(
            w,
            format!(
                "a run of {} for this pull request at this head is already recorded",
                w.name
            ),
        )),
        OpenClaim::Recorded { run, cancelled } => {
            if let Some(why) = cancelled.and_then(|old| stop_agent(cx, store, &old)) {
                cx.warn(moor_core::Warning {
                    code: "cancelled_agent_running".to_owned(),
                    message: why,
                });
            }
            let name = job.name.as_str().to_owned();
            start(job.claimed(run))?;
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
pub(crate) fn spawn_start(cx: &Cx, job: StartJob) -> Result<(), CommandError> {
    let owner = cx
        .detached()?
        .with_requester(job.firing.identity.clone())
        .with_viewer(Viewer::User(job.firing.owner));
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
        .or_else(|| box_name(w.id, w.name.as_str(), open.due_at, &open.cause).ok())
        .map_or_else(|| "unknown".to_owned(), |n| n.as_str().to_owned()))
}
