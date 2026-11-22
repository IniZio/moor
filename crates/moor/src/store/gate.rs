//! A Box's gates and the lines that say how far they have got.

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

    /// Appends `line` at `at` as line `n` of Box `id`'s gate lines. The key on the line number
    /// is the compare-and-set: of two appends of one number, the second fails with a unique
    /// violation and the caller reads the lines again.
    #[allow(
        dead_code,
        reason = "the checks and approvals that write lines come in a later change"
    )]
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
