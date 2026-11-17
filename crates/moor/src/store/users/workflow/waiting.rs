//! The moves of a run that waits at an approve step: into the wait, out of it, and its end.
//!
//! Each is [`Store::move_run`] compare-and-set on the run being in the phase, step and try it
//! was read in, so whichever of a report, an answer and an expiry comes first wins.

use moor_core::name::ForgeRepo;
use moor_core::seam::clock::Ts;
use moor_core::workflow::{HistoryEntry, RunEnd, RunOutcome, RunState, StepId, WorkflowId};
use moor_core::BoxId;

use super::runs::Open;
use crate::store::{Store, StoreError};

impl Store {
    /// Ends the run that waits at `on` with `outcome`, appending `lines` first. False, with
    /// nothing changed, when the run is no longer waiting at exactly that step and try, so an
    /// answer, an expiry and a lost Box settle once between them.
    pub(crate) fn end_waiting_run(
        &self,
        key: (WorkflowId, &ForgeRepo, Ts),
        on: (&StepId, u8),
        (box_id, outcome): (BoxId, RunOutcome),
        (lines, at): (&[HistoryEntry], Ts),
    ) -> Result<bool, StoreError> {
        let to = RunState::Done(RunEnd::Ended {
            box_id,
            ended_at: at,
            outcome,
        });
        self.move_run(key, (Open::Waiting, &to), (Some(on), lines, at, true))
    }

    /// Moves the running run, at `on`, to wait at the approve step `to`, appending `lines`, which
    /// carry the `Entered` line. False, with nothing changed, when it is not running at exactly
    /// `on`. `to` may be `on` itself, for a run that began at an approve step.
    pub(crate) fn wait_workflow_run(
        &self,
        key: (WorkflowId, &ForgeRepo, Ts),
        (on, to): ((&StepId, u8), (&StepId, u8)),
        box_id: BoxId,
        (lines, at): (&[HistoryEntry], Ts),
    ) -> Result<bool, StoreError> {
        let state = RunState::Waiting {
            box_id,
            at: to.0.clone(),
            try_n: to.1,
            since: at,
        };
        self.move_run(key, (Open::Running, &state), (Some(on), lines, at, false))
    }

    /// Moves the run that waits at `on` to `to`, a step of the run: running there, or waiting
    /// at another approve step. `lines` carry the `Answered` line and the `Entered` line. False,
    /// with nothing changed, when the run no longer waits at exactly `on`, so two answers, or an
    /// answer and an expiry, apply once.
    pub(crate) fn answer_workflow_run(
        &self,
        key: (WorkflowId, &ForgeRepo, Ts),
        on: (&StepId, u8),
        to: &RunState,
        (lines, at): (&[HistoryEntry], Ts),
    ) -> Result<bool, StoreError> {
        self.move_run(key, (Open::Waiting, to), (Some(on), lines, at, false))
    }
}
