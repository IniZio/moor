//! A Box's gates and the lines that say how far they have got.

use std::collections::HashSet;

use moor_core::domain::event::{Event, WorkflowEvent};
use moor_core::gate::{GateLine, Gates};
use moor_core::seam::clock::Ts;
use moor_core::BoxId;
use moor_schema::tables::{BoxGateLines, Boxes};
use sea_query::{Expr, ExprTrait as _, Order, Query};

use crate::store::apply::insert_event;
use crate::store::db::{DbExt as _, OptionalExtension};
use crate::store::users::workflow::run_of_box_in;
use crate::store::{RunOfBox, Store, StoreError};

/// What became of a job's line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Appended {
    /// It was written.
    Written,
    /// The Box's workflow run had ended, so nothing was written.
    RunEnded,
}

impl Store {
    /// Box `id`'s gates; `None` for a Box that has none.
    pub(crate) fn box_gates(&self, id: BoxId) -> Result<Option<Gates>, StoreError> {
        let text: Option<Option<String>> = self
            .reader()?
            .row(
                Query::select()
                    .column(Boxes::Gates)
                    .from(Boxes::Table)
                    .and_where(Expr::col(Boxes::Id).eq(id.get())),
                |r| r.get(0),
            )
            .optional()?;
        text.flatten()
            .map(|t| Gates::from_json(&t).map_err(|e| StoreError::Corrupt(format!("gates: {e}"))))
            .transpose()
    }

    /// The ids of the Boxes that have gates, open or not.
    pub(crate) fn gated_box_ids(&self) -> Result<HashSet<BoxId>, StoreError> {
        self.reader()?
            .rows(
                Query::select()
                    .column(Boxes::Id)
                    .from(Boxes::Table)
                    .and_where(Expr::col(Boxes::Gates).is_not_null()),
                |r| r.get::<_, i64>(0),
            )?
            .map(|id| Ok(BoxId::new(id?)))
            .collect()
    }

    /// When line `n` of Box `id`'s gate lines was written; none when there is no such line.
    pub(crate) fn gate_line_at(&self, id: BoxId, n: i64) -> Result<Option<Ts>, StoreError> {
        let at: Option<i64> = self
            .reader()?
            .row(
                Query::select()
                    .column(BoxGateLines::At)
                    .from(BoxGateLines::Table)
                    .and_where(Expr::col(BoxGateLines::Box).eq(id.get()))
                    .and_where(Expr::col(BoxGateLines::N).eq(n)),
                |r| r.get(0),
            )
            .optional()?;
        Ok(at.map(Ts::from_millis))
    }

    /// Appends `line` at `at` as line `n` of Box `id`'s gate lines. The key on the line number
    /// is the compare-and-set: of two appends of one number, the second fails with a unique
    /// violation and the caller reads the lines again. A line that makes a gate wait for a person
    /// writes the Box's `gate_waiting` event with it.
    pub(crate) fn append_gate_line(
        &self,
        id: BoxId,
        n: i64,
        at: Ts,
        line: &GateLine,
    ) -> Result<(), StoreError> {
        self.append_line(id, (n, at), line, false).map(drop)
    }

    /// Appends a line as [`Self::append_gate_line`] does, unless Box `id`'s workflow run has
    /// ended. The run is read in the transaction that writes, which holds the store's write lock,
    /// so a run that ends while a job runs is seen.
    pub(crate) fn append_job_line(
        &self,
        id: BoxId,
        n: i64,
        at: Ts,
        line: &GateLine,
    ) -> Result<Appended, StoreError> {
        self.append_line(id, (n, at), line, true)
    }

    /// Appends `line` as line `n` at `at`, writing nothing when `open_run` asks for an open run
    /// and the Box's has ended.
    fn append_line(
        &self,
        id: BoxId,
        (n, at): (i64, Ts),
        line: &GateLine,
        open_run: bool,
    ) -> Result<Appended, StoreError> {
        let mut writer = self.writer();
        let tx = writer.immediate()?;
        if open_run && matches!(run_of_box_in(&tx, id)?, RunOfBox::Ended(_)) {
            return Ok(Appended::RunEnded);
        }
        tx.run(
            Query::insert()
                .into_table(BoxGateLines::Table)
                .columns([
                    BoxGateLines::Box,
                    BoxGateLines::N,
                    BoxGateLines::At,
                    BoxGateLines::Entry,
                ])
                .values_panic([
                    id.get().into(),
                    n.into(),
                    at.as_millis().into(),
                    serde_json::to_string(line)?.into(),
                ]),
        )?;
        waiting::note(&tx, id, at)?;
        history::note(&tx, id, at)?;
        tx.commit()?;
        drop(writer);
        Ok(Appended::Written)
    }

    /// Writes the `gate_waiting` event of Box `id` when its gates wait for a person and the wait has
    /// none yet. It is the way a gate that waits before any line exists, such as one that only
    /// asks the owner, tells the person; calling it again for the same wait writes nothing.
    pub(crate) fn note_gate_wait(&self, id: BoxId, at: Ts) -> Result<(), StoreError> {
        let mut writer = self.writer();
        let tx = writer.immediate()?;
        waiting::ensure(&tx, id, at)?;
        tx.commit()?;
        drop(writer);
        Ok(())
    }

    /// Writes Box `id`'s `report_refused` event: moor did not accept the agent's report of
    /// `step`, and `reason` is the reply that says why.
    pub(crate) fn note_report_refused(
        &self,
        id: BoxId,
        at: Ts,
        (step, reason): (&str, &str),
    ) -> Result<(), StoreError> {
        let event = Event::Workflow(WorkflowEvent::ReportRefused {
            step: step.to_owned(),
            reason: reason.to_owned(),
        });
        let mut writer = self.writer();
        let tx = writer.immediate()?;
        insert_event(&tx, id, &event, at)?;
        tx.commit()?;
        drop(writer);
        Ok(())
    }

    /// Box `id`'s gate lines, oldest first.
    pub(crate) fn gate_lines(&self, id: BoxId) -> Result<Vec<GateLine>, StoreError> {
        self.reader()?
            .rows(
                Query::select()
                    .column(BoxGateLines::Entry)
                    .from(BoxGateLines::Table)
                    .and_where(Expr::col(BoxGateLines::Box).eq(id.get()))
                    .order_by(BoxGateLines::N, Order::Asc),
                |r| r.get::<_, String>(0),
            )?
            .map(|row| {
                serde_json::from_str(&row?)
                    .map_err(|e| StoreError::Corrupt(format!("gate line: {e}")))
            })
            .collect()
    }
}

mod history;
#[cfg(test)]
mod tests;
mod waiting;
