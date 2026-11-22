//! A Box's gates and the lines that say how far they have got.

use std::collections::HashSet;

use moor_core::gate::{GateLine, Gates};
use moor_core::seam::clock::Ts;
use moor_core::BoxId;
use moor_schema::tables::{BoxGateLines, Boxes};
use sea_query::{Expr, ExprTrait as _, Order, Query};

use crate::store::db::{DbExt as _, OptionalExtension};
use crate::store::{Store, StoreError};

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
    /// violation and the caller reads the lines again.
    pub(crate) fn append_gate_line(
        &self,
        id: BoxId,
        n: i64,
        at: Ts,
        line: &GateLine,
    ) -> Result<(), StoreError> {
        self.writer().run(
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

#[cfg(test)]
mod tests;
