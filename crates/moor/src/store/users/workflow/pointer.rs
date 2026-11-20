//! A run's frozen plan, and the move of its step pointer.

use moor_core::seam::clock::Ts;
use moor_core::workflow::{HistoryEntry, Plan, RunId, StepId};
use moor_schema::tables::WorkflowRuns;
use sea_query::{Expr, ExprTrait as _, Query};

use super::corrupt;
use super::history::append_history;
use super::row::STARTED;
use crate::store::db::{DbExt as _, OptionalExtension as _};
use crate::store::{Store, StoreError};

impl Store {
    /// The plan frozen into run `run` when it was claimed. A run with none, from before plans or
    /// of a workflow nobody planned, reads as [`Plan::single`] of `prompt`.
    pub(crate) fn run_plan(&self, run: RunId, prompt: &str) -> Result<Plan, StoreError> {
        let frozen: Option<Option<String>> = self
            .reader()?
            .row(
                Query::select()
                    .column(WorkflowRuns::Plan)
                    .from(WorkflowRuns::Table)
                    .and_where(Expr::col(WorkflowRuns::Id).eq(run.get())),
                |r| r.get(0),
            )
            .optional()?;
        frozen.flatten().map_or_else(
            || Ok(Plan::single(prompt)),
            |json| {
                Plan::parse(&json, prompt).map_err(|e| {
                    let why: Vec<String> = e.iter().map(ToString::to_string).collect();
                    corrupt("run plan", why.join("; "))
                })
            },
        )
    }

    /// Moves the pointer of the running run `run` from step
    /// `from` at try `from.1` to `to`, appending `lines` in the same transaction. False, with
    /// nothing changed, when the run is no longer running at exactly that step and try, so a
    /// move repeated or raced by another tick applies once.
    pub(crate) fn step_workflow_run(
        &self,
        run: RunId,
        (from, to): ((&StepId, u8), (&StepId, u8)),
        (lines, at): (&[HistoryEntry], Ts),
    ) -> Result<bool, StoreError> {
        self.numbered(|tx, seq| {
            let changed = tx.run(
                Query::update()
                    .table(WorkflowRuns::Table)
                    .value(WorkflowRuns::Step, to.0.as_str())
                    .value(WorkflowRuns::TryN, i64::from(to.1))
                    .value(WorkflowRuns::Seq, seq)
                    .and_where(Expr::col(WorkflowRuns::Id).eq(run.get()))
                    .and_where(Expr::col(WorkflowRuns::Outcome).eq(STARTED))
                    .and_where(Expr::col(WorkflowRuns::Step).eq(from.0.as_str()))
                    .and_where(Expr::col(WorkflowRuns::TryN).eq(i64::from(from.1))),
            )?;
            if changed == 1 {
                for line in lines {
                    append_history(tx, run.get(), at, line)?;
                }
            }
            Ok(changed)
        })
    }

    /// The lines of run `run`'s history this build knows, oldest first, each with when it was
    /// written. A line of a kind it does not know is left out.
    pub(crate) fn run_entries(&self, run: RunId) -> Result<Vec<(Ts, HistoryEntry)>, StoreError> {
        Ok(self
            .run_history(run)?
            .into_iter()
            .filter_map(|l| Some((l.at, serde_json::from_value(l.entry).ok()?)))
            .collect())
    }
}
