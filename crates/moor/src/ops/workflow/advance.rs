//! Settling the started runs of a workflow when their Box goes quiet: a run at a step before its
//! last is sent on to the next step, and a run at its last step is delivered. A Box that is
//! gone ends the run, and one no agent can run in refuses it.

use std::collections::HashMap;

use moor_core::decide::workflow::{quiet_move, quiet_since, run_verdict, QuietMove, RunVerdict};
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::request::BoxTarget;
use moor_core::seam::clock::Ts;
use moor_core::workflow::{
    HistoryEntry, Plan, RunOutcome, RunState, StepId, Workflow, WorkflowRun,
};
use moor_core::{BoxId, SteerReq};

use super::settle::{box_facts, broken_detail};
use crate::cx::Cx;
use crate::ops::steer;
use crate::store::{BoxRow, Store, StoreError};

/// Settles each started run of `w` whose Box is over or quiet: sent on to its next step,
/// delivered, failed, or refused when no agent can run in it until a person acts. The Box stays
/// open for inspection and nothing retries it.
pub(super) fn finish_runs(
    cx: &Cx,
    store: &Store,
    w: &Workflow,
    now: Ts,
    out: &mut Vec<String>,
) -> Result<(), StoreError> {
    for run in store.open_workflow_runs(w.id)? {
        let RunState::Running {
            box_id,
            ref at,
            try_n,
        } = run.state
        else {
            continue;
        };
        let row = store.box_row(box_id)?;
        let key = (w.id, &run.repo, run.due_at);
        match run_verdict(row.as_ref().map(box_facts).as_ref(), now) {
            RunVerdict::Running => {}
            RunVerdict::Finished => {
                let at = (at, try_n);
                quiet((cx, store, w), &run, (box_id, at, row.as_ref()), now, out)?;
            }
            RunVerdict::Broken => {
                if let Some(row) = &row {
                    store.refuse_run(key, (&broken_detail(row), false), now)?;
                }
            }
        }
    }
    Ok(())
}

/// Whether `row` is a Box that can still be steered.
fn open(row: Option<&BoxRow>) -> Option<&BoxRow> {
    row.filter(|r| !matches!(r.lifecycle, BoxLifecycle::Closed | BoxLifecycle::Rejected))
}

/// A started run whose Box is quiet, with what the tick knows of it.
struct Quiet<'a> {
    /// The controller context.
    cx: &'a Cx,
    /// The store.
    store: &'a Store,
    /// The workflow.
    w: &'a Workflow,
    /// The run.
    run: &'a WorkflowRun,
    /// The run's Box, if it still exists.
    row: Option<&'a BoxRow>,
    /// The step the run is at, and its try.
    at: (&'a StepId, u8),
    /// Now.
    now: Ts,
}

/// What a quiet Box means for `run`, which is at step `at`, try `at.1`, in Box `box_id`.
fn quiet(
    (cx, store, w): (&Cx, &Store, &Workflow),
    run: &WorkflowRun,
    (box_id, at, row): (BoxId, (&StepId, u8), Option<&BoxRow>),
    now: Ts,
    out: &mut Vec<String>,
) -> Result<(), StoreError> {
    let q = Quiet {
        cx,
        store,
        w,
        run,
        row,
        at,
        now,
    };
    let key = (w.id, &run.repo, run.due_at);
    let plan = store.run_plan(run.id, &w.prompt)?;
    let entries = store.run_entries(run.id)?;
    let sent = entries.iter().rev().find_map(|(when, e)| match e {
        HistoryEntry::Sent { step, try_n } if (step, *try_n) == at => Some(*when),
        _ => None,
    });
    // Right after a steer the Box still shows the quiet of the step before, so that reading
    // says nothing about this step.
    if let (Some(sent), Some(open)) = (sent, open(row)) {
        if !quiet_since(&box_facts(open), sent, now) {
            return Ok(());
        }
    }
    let mut entered: HashMap<&StepId, u8> = HashMap::new();
    for (_, e) in &entries {
        if let HistoryEntry::Entered { step, .. } = e {
            *entered.entry(step).or_default() += 1;
        }
    }
    let tries = |s: &StepId| entered.get(s).copied().unwrap_or(0);
    let end = |outcome| {
        store
            .end_workflow_run(key, (box_id, outcome), now)
            .map(|_| ())
    };
    match quiet_move(&plan, at.0, &tries) {
        QuietMove::Delivered => end(RunOutcome::Delivered {
            result: String::new(),
        }),
        QuietMove::Enter { .. } | QuietMove::Fail { .. } if open(row).is_none() => {
            // Nobody is left to send the next step to.
            end(RunOutcome::Incomplete { at: at.0.clone() })
        }
        QuietMove::Fail { at, reason, .. } => end(RunOutcome::Failed { at, reason }),
        QuietMove::Enter {
            step,
            try_n,
            exhausted,
        } => q.enter(&plan, (step, try_n), exhausted, out),
    }
}

impl Quiet<'_> {
    /// Sends the run on to `step`, for its `try_n`th entry, and records the move.
    fn enter(
        &self,
        plan: &Plan,
        (step, try_n): (StepId, u8),
        exhausted: Vec<StepId>,
        out: &mut Vec<String>,
    ) -> Result<(), StoreError> {
        let (Some(row), Some(next)) = (open(self.row), plan.step(&step)) else {
            return Ok(());
        };
        // Steer first and record after, so a crash in between sends the step twice and never
        // skips it.
        if let Err(why) = send(self.cx, row, &Plan::next_step_steer(next)) {
            out.push(format!(
                "workflow {}: could not send step {step} to {}: {why}",
                self.w.name,
                row.name.as_str()
            ));
            return Ok(());
        }
        let mut lines: Vec<HistoryEntry> = exhausted
            .into_iter()
            .map(|step| HistoryEntry::Exhausted { step })
            .collect();
        lines.push(HistoryEntry::Entered {
            step: step.clone(),
            try_n,
        });
        lines.push(HistoryEntry::Sent {
            step: step.clone(),
            try_n,
        });
        let key = (self.w.id, &self.run.repo, self.run.due_at);
        self.store
            .step_workflow_run(key, (self.at, (&step, try_n)), (&lines, self.now))
            .map(|_| ())
    }
}

/// Steers the agent of `row` with `text`, as its owner does.
fn send(cx: &Cx, row: &BoxRow, text: &str) -> Result<(), String> {
    let req = SteerReq {
        target: BoxTarget::Id(row.id),
        text: Some(text.to_owned()),
        menu: None,
        interrupt: false,
    };
    cx.for_box(row.user, || steer::run(cx, &req))
        .map(|_| ())
        .map_err(|e| e.error.to_string())
}
