//! The workflow tick: finishes, expires and fires stored workflows, and records each outcome.
//!
//! A firing is claimed in the store before its Box exists. The claim is a row unique on
//! workflow, repo and due time, so only one tick ever wins it, and nothing but the winner
//! starts a Box. The start settles the claim afterwards. A crash between the two leaves the
//! claim, which [`resolve_interrupted`] settles from the Box's name and never by starting again:
//! a missed run costs nothing and a repeated one costs a Box's tokens.

use moor_core::decide::workflow::{
    run_verdict, workflow_breaker, workflow_due, RunVerdict, WorkflowVerdict,
};
use moor_core::name::ForgeRepo;
use moor_core::seam::clock::Ts;
use moor_core::user::{Identity, UserId};
use moor_core::workflow::{
    box_name, NewRun, Plan, RunEnd, RunState, StepId, Workflow, WorkflowId, WorkflowState,
};
use moor_core::{BoxId, BoxName, StartReq};

use crate::cx::Cx;
use crate::ops::start;
use crate::store::{Store, StoreError};
use advance::finish_runs;
pub(crate) use check::{run_check, CheckJob};
use facts::history;
use previous::close_previous;
use settle::{box_facts, broken_detail, close_open_box, enforce_start_limit, expire_runs};

/// One claimed firing, ready to start a Box.
#[derive(Debug, Clone)]
pub(crate) struct StartJob {
    /// The workflow.
    pub(crate) workflow: WorkflowId,
    /// Whose Box it is.
    pub(crate) owner: UserId,
    /// An identity of the owner, which the start acts as.
    pub(crate) identity: Identity,
    /// The repo.
    pub(crate) repo: ForgeRepo,
    /// The repos the Box clones beside `repo`.
    pub(crate) also: Vec<ForgeRepo>,
    /// The override each Box starts with.
    pub(crate) config: Option<String>,
    /// The due time the claim answers.
    pub(crate) due_at: Ts,
    /// The Box to start.
    pub(crate) name: BoxName,
    /// What the Box's agent is asked to do.
    pub(crate) prompt: String,
    /// The step of the workflow's plan the run starts at.
    pub(crate) first_step: StepId,
}

impl StartJob {
    /// The claim's key.
    const fn key(&self) -> (WorkflowId, &ForgeRepo, Ts) {
        (self.workflow, &self.repo, self.due_at)
    }
}

/// Runs one pass over every workflow that fires or has a run open, handing each claimed firing
/// to `launch`, which must not wait for the start. Returns what to log.
pub(crate) fn tick(
    cx: &Cx,
    (launch, checks): (&dyn Fn(StartJob), &dyn Fn(CheckJob)),
    draining: &dyn Fn() -> bool,
) -> Vec<String> {
    let mut out = Vec::new();
    let workflows = cx
        .store()
        .map_err(|e| e.error.to_string())
        .and_then(|s| s.ticked_workflows().map_err(|e| e.to_string()));
    match workflows {
        Ok(list) => {
            for w in list.iter().take_while(|_| !draining()) {
                if let Err(e) = tick_one(cx, w, (launch, checks), &mut out) {
                    out.push(format!("workflow {}: {e}", w.name));
                }
            }
        }
        Err(e) => out.push(e),
    }
    out
}

/// One workflow's pass: finish, expire, check the breaker, then fire what is due. A disabled
/// workflow only settles the runs it has open.
fn tick_one(
    cx: &Cx,
    w: &Workflow,
    (launch, checks): (&dyn Fn(StartJob), &dyn Fn(CheckJob)),
    out: &mut Vec<String>,
) -> Result<(), StoreError> {
    let store = cx
        .store()
        .map_err(|e| StoreError::Invalid(e.error.to_string()))?;
    let now = cx.clock().now();
    finish_runs(cx, store, w, now, (checks, out))?;
    expire_runs(cx, store, w, now, out)?;
    enforce_start_limit(cx, store, w, now, out)?;
    if matches!(w.state, WorkflowState::Disabled { .. }) {
        return Ok(());
    }
    let history = history(store, w)?;
    if let Some(trip) = workflow_breaker(w, &history.unskipped) {
        if store.disable_workflow(w.id, &trip.reason)? {
            out.push(format!("workflow {} disabled: {}", w.name, trip.reason));
        }
        return Ok(());
    }
    let verdicts = workflow_due(w, &history.facts(store)?, now);
    for v in verdicts {
        match v.verdict {
            WorkflowVerdict::NotDue => {}
            WorkflowVerdict::SkipOverlap { due_at } => {
                record(
                    store,
                    w,
                    &v.repo,
                    due_at,
                    now,
                    (RunState::Done(RunEnd::SkippedOverlap), None),
                )?;
            }
            WorkflowVerdict::Start { due_at } => {
                claim(store, w, &v.repo, due_at, now, launch)?;
            }
        }
    }
    Ok(())
}

/// Records a run that never starts a Box; a repeated record changes nothing.
fn record(
    store: &Store,
    w: &Workflow,
    repo: &ForgeRepo,
    due_at: Ts,
    now: Ts,
    (state, plan): (RunState, Option<Plan>),
) -> Result<bool, StoreError> {
    store.record_workflow_run(&NewRun {
        workflow: w.id,
        repo: repo.clone(),
        due_at,
        state,
        recorded_at: now,
        plan,
    })
}

/// Claims the firing and hands it to `launch`; a refusal before any Box is recorded as such.
fn claim(
    store: &Store,
    w: &Workflow,
    repo: &ForgeRepo,
    due_at: Ts,
    now: Ts,
    launch: &dyn Fn(StartJob),
) -> Result<(), StoreError> {
    let refuse = |detail: String| {
        record(
            store,
            w,
            repo,
            due_at,
            now,
            (RunState::Done(RunEnd::Refused { detail }), None),
        )
        .map(|_| ())
    };
    let name = match box_name(w.id, w.name.as_str(), repo, due_at) {
        Ok(name) => name,
        Err(e) => return refuse(e.to_string()),
    };
    let Some(identity) = store.identities(w.owner)?.into_iter().next() else {
        return refuse("the owner has no identity to start a Box as".to_owned());
    };
    // A workflow nobody planned freezes none, and its Box gets the bare prompt as it always has.
    let planned = !w.plan.is_single_of(&w.prompt);
    let frozen = planned.then(|| w.plan.clone());
    let prompt = if planned {
        w.plan.start_prompt(&w.prompt)
    } else {
        w.prompt.clone()
    };
    if record(store, w, repo, due_at, now, (RunState::Queued, frozen))? {
        launch(StartJob {
            workflow: w.id,
            owner: w.owner,
            identity,
            repo: repo.clone(),
            also: w.also.clone(),
            config: w.config.clone(),
            due_at,
            name,
            prompt,
            first_step: w.plan.first().id.clone(),
        });
    }
    Ok(())
}

/// Starts the Box of a claimed firing as the owner `cx` acts for, through the start a person
/// uses, and settles the claim with its Box or its refusal. Returns what to log.
pub(crate) fn run_start(cx: &Cx, job: &StartJob) -> Vec<String> {
    let state = match cx.store() {
        Ok(store) => start_box(cx, store, job),
        Err(e) => RunState::Done(RunEnd::Refused {
            detail: e.error.to_string(),
        }),
    };
    // A start the drain cut may have left its Box going, so the next process settles the claim.
    if matches!(state, RunState::Done(_)) && cx.env().draining().is_cancelled() {
        return cx.take_warnings().into_iter().map(|w| w.message).collect();
    }
    let settled = cx.store().map_err(|e| e.error.to_string()).and_then(|s| {
        s.resolve_workflow_start(job.key(), &state, cx.clock().now())
            .map_err(|e| e.to_string())
    });
    let mut out: Vec<String> = cx.take_warnings().into_iter().map(|w| w.message).collect();
    if let (Ok(true), RunState::Running { box_id, .. }) = (&settled, &state) {
        if let Ok(store) = cx.store() {
            out.extend(close_previous(cx, store, job, *box_id));
            out.extend(cx.take_warnings().into_iter().map(|w| w.message));
        }
    }
    if matches!(settled, Ok(false)) {
        let late = cx.store().map_err(|e| e.error.to_string());
        if let Err(why) = late.and_then(|s| close_open_box(cx, s, &job.name)) {
            out.push(format!(
                "could not close the late Box {}: {why}",
                job.name.as_str()
            ));
        }
    }
    if let Err(e) = settled {
        out.push(format!(
            "could not record the start of {}: {e}",
            job.name.as_str()
        ));
    }
    out
}

/// Starts the Box, or says why not.
fn start_box(cx: &Cx, store: &Store, job: &StartJob) -> RunState {
    let refused = |detail: String| RunState::Done(RunEnd::Refused { detail });
    match store.open_box_by_name(&job.name) {
        Ok(Some(_)) => {
            return refused(format!("a Box named {} is already open", job.name.as_str()))
        }
        Ok(None) => {}
        Err(e) => return refused(e.to_string()),
    }
    let req = StartReq {
        cold: false,
        config: job.config.clone(),
        name: job.name.clone(),
        prompt: Some(job.prompt.clone()),
        issue: None,
        base: None,
        model: None,
        node: None,
        no_agent: false,
        resume: false,
        repo: Some(job.repo.clone()),
        also: Some(job.also.clone()),
        parent: None,
        identity: None,
    };
    match start::run(cx, &req) {
        Ok(view) => RunState::Running {
            box_id: BoxId::new(view.boxed.id),
            at: job.first_step.clone(),
            try_n: 1,
        },
        Err(e) => refused(e.error.to_string()),
    }
}

/// Settles each claim an earlier process left, never starting a Box: one whose Box is open
/// becomes its run, any other is recorded as refused. Returns what to log.
pub(crate) fn resolve_interrupted(cx: &Cx) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(store) = cx.store() else { return out };
    let claims = store.starting_workflow_runs().unwrap_or_default();
    for run in claims {
        let workflow = store.workflow_by_id(run.workflow).ok().flatten();
        let found = workflow
            .as_ref()
            .and_then(|w| box_name(w.id, w.name.as_str(), &run.repo, run.due_at).ok())
            .and_then(|name| store.open_box_by_name(&name).ok().flatten());
        let now = cx.clock().now();
        let state = match (found, workflow) {
            (Some(row), Some(w)) => {
                match box_facts(store, &row).map(|f| run_verdict(Some(&f), now)) {
                    Ok(RunVerdict::Broken) => RunState::Done(RunEnd::Refused {
                        detail: broken_detail(&row),
                    }),
                    _ => RunState::Running {
                        box_id: row.id,
                        at: store
                            .run_plan(run.id, &w.prompt)
                            .map_or_else(|_| w.plan.first().id.clone(), |p| p.first().id.clone()),
                        try_n: 1,
                    },
                }
            }
            _ => RunState::Done(RunEnd::Refused {
                detail: "the controller stopped before the Box started".to_owned(),
            }),
        };
        let key = (run.workflow, &run.repo, run.due_at);
        if let Err(e) = store.resolve_workflow_start(key, &state, now) {
            out.push(format!(
                "could not settle a claimed run of {}: {e}",
                run.repo
            ));
        }
    }
    out
}

mod advance;
mod check;
mod facts;
mod previous;
mod settle;
#[cfg(test)]
mod tests;
