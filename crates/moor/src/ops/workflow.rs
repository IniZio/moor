//! The workflow tick: finishes, expires and fires stored workflows, and records each outcome.
//!
//! A firing is claimed in the store before its Box exists. The claim is a row unique on
//! workflow, repo and due time, so only one tick ever wins it, and nothing but the winner
//! starts a Box. The start settles the claim afterwards. A crash between the two leaves the
//! claim, which [`resolve_interrupted`] settles from the Box's name and never by starting again:
//! a missed run costs nothing and a repeated one costs a Box's tokens.

use moor_core::decide::workflow::{
    run_finished, workflow_breaker, workflow_due, workflow_expired, PriorRun, RunBoxFacts,
    WorkflowRepoFacts, WorkflowVerdict,
};
use moor_core::name::ForgeRepo;
use moor_core::request::EndSessionReq;
use moor_core::seam::clock::Ts;
use moor_core::user::{Identity, UserId};
use moor_core::workflow::{box_name, RunEnd, Workflow, WorkflowId, WorkflowOutcome, WorkflowRun};
use moor_core::{BoxId, BoxName, StartReq};

use crate::cx::Cx;
use crate::ops::end_session;
use crate::ops::start;
use crate::store::{BoxRow, Store, StoreError};

/// How many of a repo's latest runs the tick reads: enough for the breaker to see past skips.
const HISTORY: u64 = 20;

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
    /// The due time the claim answers.
    pub(crate) due_at: Ts,
    /// The Box to start.
    pub(crate) name: BoxName,
    /// What the Box's agent is asked to do.
    pub(crate) prompt: String,
}

impl StartJob {
    /// The claim's key.
    const fn key(&self) -> (WorkflowId, &ForgeRepo, Ts) {
        (self.workflow, &self.repo, self.due_at)
    }
}

/// Runs one pass over every enabled workflow, handing each claimed firing to `launch`, which
/// must not wait for the start. Returns what to log.
pub(crate) fn tick(cx: &Cx, launch: &dyn Fn(StartJob), draining: &dyn Fn() -> bool) -> Vec<String> {
    let mut out = Vec::new();
    let workflows = cx
        .store()
        .map_err(|e| e.error.to_string())
        .and_then(|s| s.enabled_workflows().map_err(|e| e.to_string()));
    match workflows {
        Ok(list) => {
            for w in list.iter().take_while(|_| !draining()) {
                if let Err(e) = tick_one(cx, w, launch, &mut out) {
                    out.push(format!("workflow {}: {e}", w.name));
                }
            }
        }
        Err(e) => out.push(e),
    }
    out
}

/// One workflow's pass: finish, expire, check the breaker, then fire what is due.
fn tick_one(
    cx: &Cx,
    w: &Workflow,
    launch: &dyn Fn(StartJob),
    out: &mut Vec<String>,
) -> Result<(), StoreError> {
    let store = cx
        .store()
        .map_err(|e| StoreError::Invalid(e.error.to_string()))?;
    let now = cx.clock().now();
    finish_runs(store, w, now)?;
    expire_runs(cx, store, w, now, out)?;
    let history = history(store, w)?;
    if let Some(trip) = workflow_breaker(w, &history.runs) {
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
                    WorkflowOutcome::SkippedOverlap,
                )?;
            }
            WorkflowVerdict::Start { due_at } => {
                claim(store, w, &v.repo, due_at, now, launch)?;
            }
        }
    }
    Ok(())
}

/// Each repo's latest runs.
struct History {
    /// The workflow's runs of every repo, newest first within each repo.
    runs: Vec<WorkflowRun>,
}

impl History {
    /// The decider's facts: each repo's latest due time and its latest real run. Skipped and
    /// refused runs are looked through, so a skip does not unblock the next firing.
    fn facts(&self, store: &Store) -> Result<Vec<WorkflowRepoFacts>, StoreError> {
        let mut facts: Vec<WorkflowRepoFacts> = Vec::new();
        for run in &self.runs {
            if facts.iter().any(|f| f.repo == run.repo) {
                continue;
            }
            let mine = || self.runs.iter().filter(|r| r.repo == run.repo);
            let prior = match mine().find_map(|r| real_run(&r.outcome)) {
                Some(Real::Prior(p)) => Some(p),
                Some(Real::TimedOut(id)) => Some(PriorRun::TimedOut {
                    box_open: box_is_open(store, id)?,
                }),
                None => None,
            };
            facts.push(WorkflowRepoFacts {
                repo: run.repo.clone(),
                last_due: mine().map(|r| r.due_at).max(),
                prior,
            });
        }
        Ok(facts)
    }
}

/// A run that is a real attempt, as the decider sees it.
enum Real {
    /// Settled without looking at a Box.
    Prior(PriorRun),
    /// Timed out; its Box decides.
    TimedOut(BoxId),
}

/// `outcome` as a real attempt; none for a skipped or refused run.
const fn real_run(outcome: &WorkflowOutcome) -> Option<Real> {
    match outcome {
        WorkflowOutcome::Starting => Some(Real::Prior(PriorRun::Starting)),
        WorkflowOutcome::Started { .. } => Some(Real::Prior(PriorRun::Started)),
        WorkflowOutcome::Finished { .. } => Some(Real::Prior(PriorRun::Finished)),
        WorkflowOutcome::TimedOut { box_id, .. } => Some(Real::TimedOut(*box_id)),
        WorkflowOutcome::SkippedOverlap | WorkflowOutcome::Refused { .. } => None,
    }
}

/// Reads each repo's latest runs of `w`.
fn history(store: &Store, w: &Workflow) -> Result<History, StoreError> {
    let mut runs = Vec::new();
    for repo in &w.repos {
        runs.extend(store.recent_workflow_runs(w.id, repo, HISTORY)?);
    }
    Ok(History { runs })
}

/// Whether Box `id` exists and is neither closed nor rejected.
fn box_is_open(store: &Store, id: BoxId) -> Result<bool, StoreError> {
    use moor_core::domain::box_state::BoxLifecycle;
    Ok(store
        .box_row(id)?
        .is_some_and(|r| !matches!(r.lifecycle, BoxLifecycle::Closed | BoxLifecycle::Rejected)))
}

/// Records each started run of `w` whose Box is over as finished.
fn finish_runs(store: &Store, w: &Workflow, now: Ts) -> Result<(), StoreError> {
    for run in store.open_workflow_runs(w.id)? {
        let WorkflowOutcome::Started { box_id } = run.outcome else {
            continue;
        };
        let row = store.box_row(box_id)?;
        let facts = row.as_ref().map(|r| RunBoxFacts {
            lifecycle: r.lifecycle,
            session: r.session,
            idle_since: r.idle_since,
        });
        if run_finished(facts.as_ref(), now) {
            store.end_workflow_run((w.id, &run.repo, run.due_at), RunEnd::Finished, now)?;
        }
    }
    Ok(())
}

/// Ends the agent session of each started run past its timeout, keeps the Box, and records the
/// run as timed out whether or not a session was live to end.
fn expire_runs(
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

/// Records a run that never starts a Box; a repeated record changes nothing.
fn record(
    store: &Store,
    w: &Workflow,
    repo: &ForgeRepo,
    due_at: Ts,
    now: Ts,
    outcome: WorkflowOutcome,
) -> Result<bool, StoreError> {
    store.record_workflow_run(&WorkflowRun {
        workflow: w.id,
        repo: repo.clone(),
        due_at,
        outcome,
        recorded_at: now,
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
            WorkflowOutcome::Refused { detail },
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
    if record(store, w, repo, due_at, now, WorkflowOutcome::Starting)? {
        launch(StartJob {
            workflow: w.id,
            owner: w.owner,
            identity,
            repo: repo.clone(),
            due_at,
            name,
            prompt: w.prompt.clone(),
        });
    }
    Ok(())
}

/// Starts the Box of a claimed firing as the owner `cx` acts for, through the start a person
/// uses, and settles the claim with its Box or its refusal. Returns what to log.
pub(crate) fn run_start(cx: &Cx, job: &StartJob) -> Vec<String> {
    let outcome = match cx.store() {
        Ok(store) => start_box(cx, store, job),
        Err(e) => WorkflowOutcome::Refused {
            detail: e.error.to_string(),
        },
    };
    let settled = cx.store().map_err(|e| e.error.to_string()).and_then(|s| {
        s.resolve_workflow_start(job.key(), &outcome)
            .map_err(|e| e.to_string())
    });
    let mut out: Vec<String> = cx.take_warnings().into_iter().map(|w| w.message).collect();
    if let Err(e) = settled {
        out.push(format!(
            "could not record the start of {}: {e}",
            job.name.as_str()
        ));
    }
    out
}

/// Starts the Box, or says why not.
fn start_box(cx: &Cx, store: &Store, job: &StartJob) -> WorkflowOutcome {
    let refused = |detail: String| WorkflowOutcome::Refused { detail };
    match store.open_box_by_name(&job.name) {
        Ok(Some(_)) => {
            return refused(format!("a Box named {} is already open", job.name.as_str()))
        }
        Ok(None) => {}
        Err(e) => return refused(e.to_string()),
    }
    let req = StartReq {
        name: job.name.clone(),
        prompt: Some(job.prompt.clone()),
        issue: None,
        base: None,
        model: None,
        node: None,
        no_agent: false,
        resume: false,
        repo: Some(job.repo.clone()),
        also: None,
        parent: None,
        identity: None,
    };
    match start::run(cx, &req) {
        Ok(view) => WorkflowOutcome::Started {
            box_id: BoxId::new(view.boxed.id),
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
        let found = store
            .workflow_by_id(run.workflow)
            .ok()
            .flatten()
            .and_then(|w| box_name(w.id, w.name.as_str(), &run.repo, run.due_at).ok())
            .and_then(|name| store.open_box_by_name(&name).ok().flatten());
        let outcome = found.map_or_else(
            || WorkflowOutcome::Refused {
                detail: "the controller stopped before the Box started".to_owned(),
            },
            |row: BoxRow| WorkflowOutcome::Started { box_id: row.id },
        );
        let key = (run.workflow, &run.repo, run.due_at);
        if let Err(e) = store.resolve_workflow_start(key, &outcome) {
            out.push(format!(
                "could not settle a claimed run of {}: {e}",
                run.repo
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests;
