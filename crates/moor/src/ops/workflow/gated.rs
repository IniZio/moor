//! The run of a Box with gates: the gates, and never the agent going quiet, say when it is done.
//!
//! The run follows where the gates stand. All passed delivers it with the note of the agent's last
//! accepted report, a gate that failed fails it, and a gate that is stuck or asks the owner sets
//! it waiting, as an approve step does, until the gate opens for the agent again. The gates
//! themselves are worked by the gate pass, so a tick only reads them. Each move is a
//! compare-and-set on the run's step and try, so a tick raced by another changes nothing.

use moor_core::decide::workflow::{run_verdict, RunVerdict};
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::gate::{status, GateLine, GateNext, GateStatus, Gates};
use moor_core::seam::clock::Ts;
use moor_core::workflow::{
    gated_move, GatedMove, HistoryEntry, RunOutcome, RunState, Standing, StepId, Unfinished,
    WorkflowRun,
};
use moor_core::{BoxId, ErrorKind};

use super::settle::{broken_detail, fail_death_capped};
use super::wait::lose_waiting;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::quiet::{box_facts, may_tell};
use crate::store::{BoxRow, Store, StoreError};

/// A run of a Box with gates, and the step and try it was read at.
struct Held<'a> {
    /// The store.
    store: &'a Store,
    /// The run.
    run: &'a WorkflowRun,
    /// Its Box.
    box_id: BoxId,
    /// The step and try it is at.
    at: (&'a StepId, u8),
    /// Whether it waits for a person.
    waiting: bool,
    /// Now.
    now: Ts,
}

/// Settles `run` from where the gates of its Box stand, when the Box has gates; false, with
/// nothing done, when it has none, and the run is the step walker's. A run that is not working
/// or waiting is left alone.
pub(super) fn settle_held(
    cx: &Cx,
    store: &Store,
    run: &WorkflowRun,
    now: Ts,
) -> Result<bool, StoreError> {
    let (waiting, box_id, at, try_n) = match &run.state {
        RunState::Running { box_id, at, try_n } => (false, *box_id, at, *try_n),
        RunState::Waiting {
            box_id, at, try_n, ..
        } => (true, *box_id, at, *try_n),
        RunState::Queued | RunState::Done(_) => return Ok(false),
    };
    if store.box_gates(box_id)?.is_none() {
        return Ok(false);
    }
    let held = Held {
        store,
        run,
        box_id,
        at: (at, try_n),
        waiting,
        now,
    };
    held.settle(cx)?;
    Ok(true)
}

impl Held<'_> {
    /// Reads the gates and makes the move they call for.
    fn settle(&self, cx: &Cx) -> Result<(), StoreError> {
        let Some(gates) = self.store.box_gates(self.box_id)? else {
            return Ok(());
        };
        let lines = self.store.gate_lines(self.box_id)?;
        let status = status(&gates, &lines);
        let row = self.store.box_row(self.box_id)?;
        let over = matches!(
            status,
            GateStatus::Passed { .. } | GateStatus::Failed { .. }
        );
        if !over && self.cannot_go_on(row.as_ref())? {
            return Ok(());
        }
        let open = row
            .as_ref()
            .is_some_and(|r| !matches!(r.lifecycle, BoxLifecycle::Closed | BoxLifecycle::Rejected));
        let asks = matches!(
            status,
            GateStatus::Open {
                next: GateNext::Ask,
                ..
            }
        );
        let quiet = match &row {
            Some(r) if asks && open => may_tell(cx, self.store, r)?,
            _ => false,
        };
        let standing = Standing {
            at: self.at,
            waiting: self.waiting,
            box_open: open,
            quiet,
        };
        self.apply(gated_move(standing, &status), (&status, &lines))
    }

    /// Makes `m`, which the gates standing at `status` over `lines` called for.
    fn apply(
        &self,
        m: GatedMove,
        (status, lines): (&GateStatus, &[GateLine]),
    ) -> Result<(), StoreError> {
        match m {
            GatedMove::Stay => Ok(()),
            GatedMove::Deliver { outcome } => self.end(RunOutcome::Delivered {
                result: lines
                    .iter()
                    .rev()
                    .find_map(GateLine::accepted_note)
                    .unwrap_or_default()
                    .to_owned(),
                verdict: Some(outcome),
            }),
            GatedMove::Fail { gate, reason } => self.end(RunOutcome::Failed { at: gate, reason }),
            GatedMove::Incomplete => self.end(RunOutcome::Incomplete {
                at: self.at.0.clone(),
                why: Unfinished::BoxClosed,
            }),
            // The line that put the gate in hand wrote its history, so the move writes none.
            GatedMove::Enter { gate, try_n } => self
                .store
                .step_workflow_run(self.run.id, (self.at, (&gate, try_n)), (&[], self.now))
                .map(drop),
            GatedMove::Wait { gate, try_n } => {
                let (why, unanswered) = match status {
                    GateStatus::Stuck {
                        why, unanswered, ..
                    } => (Some(why.clone()), unanswered.clone()),
                    _ => (None, None),
                };
                let line = HistoryEntry::GateWaiting {
                    step: gate.clone(),
                    try_n,
                    why,
                    unanswered,
                };
                self.store
                    .wait_workflow_run(
                        self.run.id,
                        (self.at, (&gate, try_n)),
                        self.box_id,
                        (&[line], self.now),
                    )
                    .map(drop)
            }
            GatedMove::Resume { gate, try_n } => {
                let lines = [
                    HistoryEntry::Resumed {
                        step: self.at.0.clone(),
                    },
                    entered(&gate, try_n),
                ];
                let back = RunState::Running {
                    box_id: self.box_id,
                    at: gate,
                    try_n,
                };
                self.store
                    .answer_workflow_run(self.run.id, self.at, &back, (&lines, self.now))
                    .map(drop)
            }
        }
    }

    /// Ends the run with `outcome`, from the phase it is in.
    fn end(&self, outcome: RunOutcome) -> Result<(), StoreError> {
        let (key, box_id, now) = (self.run.id, self.box_id, self.now);
        if self.waiting {
            self.store
                .end_waiting_run(key, self.at, (box_id, outcome), (&[], now))
        } else {
            self.store
                .end_workflow_run(key, self.at, (box_id, outcome), (&[], now))
        }
        .map(drop)
    }

    /// Ends the run when no agent can work its gates, as a work step's run ends: a Box that no
    /// agent can run in until a person acts refuses it, and one whose places died and was not
    /// resumed fails it. A waiting run is left to [`lose_waiting`], which also ends it when its
    /// Box is gone or closed. True when it ended the run.
    fn cannot_go_on(&self, row: Option<&BoxRow>) -> Result<bool, StoreError> {
        let (store, run, now) = (self.store, self.run, self.now);
        if self.waiting {
            lose_waiting(store, run, (self.box_id, self.at.0, self.at.1), now)?;
            return Ok(false);
        }
        let Some(row) = row else { return Ok(false) };
        match run_verdict(Some(&box_facts(store, row)?), now) {
            RunVerdict::Broken => {
                store.refuse_run(run.id, (&broken_detail(store, row), false), now)?;
                Ok(true)
            }
            RunVerdict::DeathCapped => {
                fail_death_capped(store, run, (self.box_id, self.at.0, self.at.1), now)?;
                Ok(true)
            }
            RunVerdict::Running | RunVerdict::Finished => Ok(false),
        }
    }
}

/// The step a run starts at: the first gate of its Box when it has gates, else `plan_first`.
pub(super) fn first_step(gates: Option<&Gates>, plan_first: StepId) -> StepId {
    gates
        .and_then(|g| g.iter().next())
        .map_or(plan_first, |g| g.name.clone())
}

/// The line for a run that reaches `gate`, try `try_n`.
fn entered(gate: &StepId, try_n: u8) -> HistoryEntry {
    HistoryEntry::Entered {
        step: gate.clone(),
        try_n,
    }
}

/// The refusal of `answer_run` for a run that waits at `at`, which is no approve step: when the
/// Box has gates it is a gate, and the Box's gate takes the answer, not the run.
pub(super) fn not_approve(store: &Store, box_id: BoxId, at: &StepId) -> CommandError {
    let gated = store.box_gates(box_id).ok().flatten().is_some();
    let why = if gated {
        format!("the run waits at gate {at} of its Box, which answers to answer_gate on the Box, not to answer_run")
    } else {
        format!("step {at} of the plan is not an approve step")
    };
    CommandError::new(ErrorKind::Refused, why)
}
