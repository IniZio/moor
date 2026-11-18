//! `workflow-run-now`: the owner fires a workflow once, now, for some or all of its repos.
//!
//! Each repo goes through the same preparation and start a cron firing uses. The claim is a manual
//! one: it is recorded only when the repo has no open run, which the store checks in the write
//! that inserts the row. A repo that is refused records nothing.

use moor_core::name::ForgeRepo;
use moor_core::view::workflow::{RunNowLine, RunNowValue};
use moor_core::workflow::{
    box_name, NewRun, RunEnd, RunState, RunTrigger, Workflow, WorkflowName, WorkflowRun,
};
use moor_core::ErrorKind;

use super::{prepare, run_start, Prepared};
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
    (name, repos, expected_revision): (&str, &[String], Option<u64>),
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
        runs.push(fire(cx, store, &w, repo)?);
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

/// Claims a manual run of `w` for `repo` and starts its Box on the spot, as the owner `cx` acts
/// for. A claim the store refuses records nothing; a start that fails after the claim leaves a
/// refused run, as a cron firing does.
fn fire(
    cx: &Cx,
    store: &Store,
    w: &Workflow,
    repo: &ForgeRepo,
) -> Result<RunNowLine, CommandError> {
    let now = cx.clock().now();
    let Prepared { job, frozen } = match prepare(store, w, repo, (now, RunTrigger::Manual)) {
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
            run_start(cx, &job);
            let started = store
                .recent_workflow_runs(w.id, repo, 5)?
                .into_iter()
                .find(|r| r.due_at == now);
            Ok(match started.map(|r| r.state) {
                Some(RunState::Done(RunEnd::Refused { detail })) => declined(repo, detail),
                _ => RunNowLine {
                    repo: repo.to_string(),
                    status: "started".to_owned(),
                    box_name: Some(name),
                    reason: None,
                },
            })
        }
    }
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
