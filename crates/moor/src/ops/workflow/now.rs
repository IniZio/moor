//! `workflow-run-now`: the owner fires a workflow once, now, for some or all of its repos.
//!
//! Each repo goes through the same preparation and start a cron firing uses. The claim is a manual
//! one: it is recorded only when the repo has no open run, which the store checks in the write
//! that inserts the row. A repo that is refused records nothing. A recorded claim's Box starts on
//! a thread of its own, as a cron firing's does, so the answer never waits on a Box's setup; a
//! start that is refused then settles the run as refused, which `list_workflow_runs` shows.

use std::thread;

use moor_core::name::ForgeRepo;
use moor_core::user::Viewer;
use moor_core::view::workflow::{RunNowLine, RunNowValue};
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

/// Fires `user`'s workflow `name` for `repos`, each `OWNER/NAME` and one of the workflow's; every
/// repo of the workflow when there are none. `expected_revision` refuses the call when the
/// workflow changed after the caller read it.
pub(crate) fn run(
    cx: &Cx,
    store: &Store,
    user: moor_core::user::UserId,
    args: (&str, &[String], Option<u64>),
) -> Result<RunNowValue, CommandError> {
    run_with(cx, store, user, args, &|job| spawn_start(cx, job))
}

/// Like [`run`], handing each recorded claim to `start`.
pub(super) fn run_with(
    cx: &Cx,
    store: &Store,
    user: moor_core::user::UserId,
    (name, repos, expected_revision): (&str, &[String], Option<u64>),
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
    let mut chosen: Vec<ForgeRepo> = Vec::new();
    for text in repos {
        let repo =
            ForgeRepo::parse(text).map_err(|_| usage(format!("{text} must be OWNER/NAME")))?;
        if !w.repos.contains(&repo) {
            return Err(usage(format!("{name} does not run for {repo}")));
        }
        if !chosen.contains(&repo) {
            chosen.push(repo);
        }
    }
    if chosen.is_empty() {
        chosen.clone_from(&w.repos);
    }
    let mut runs = Vec::new();
    for repo in &chosen {
        runs.push(fire(cx, store, &w, repo, start)?);
    }
    Ok(RunNowValue {
        workflow: name.to_string(),
        runs,
    })
}

/// A repo that was refused, and why.
fn declined(repo: &ForgeRepo, why: String) -> RunNowLine {
    RunNowLine {
        repo: repo.to_string(),
        status: "refused".to_owned(),
        box_name: None,
        reason: Some(why),
    }
}

/// Claims a manual run of `w` for `repo` and hands it to `start`. A claim the store refuses
/// records nothing; a start that fails after the claim leaves a refused run, as a cron firing does.
fn fire(
    cx: &Cx,
    store: &Store,
    w: &Workflow,
    repo: &ForgeRepo,
    start: &dyn Fn(StartJob) -> Result<(), CommandError>,
) -> Result<RunNowLine, CommandError> {
    let now = cx.clock().now();
    let Prepared { job, frozen } = match prepare(cx, store, w, repo, (now, RunTrigger::Manual)) {
        Ok(prepared) => prepared,
        Err(why) => return Ok(declined(repo, why)),
    };
    let claim = NewRun {
        workflow: w.id,
        repo: repo.clone(),
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
                repo,
                format!("a run for {repo} is still open ({phase}, Box {box_name})"),
            ))
        }
        ManualClaim::Taken => Ok(declined(
            repo,
            format!("a run for {repo} was fired at this very moment already"),
        )),
        ManualClaim::Recorded => {
            let name = job.name.as_str().to_owned();
            start(job)?;
            Ok(RunNowLine {
                repo: repo.to_string(),
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
        .or_else(|| box_name(w.id, w.name.as_str(), &open.repo, open.due_at, open.trigger).ok())
        .map_or_else(|| "unknown".to_owned(), |n| n.as_str().to_owned()))
}
