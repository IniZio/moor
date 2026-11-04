//! Refs a Box carries, such as its branch.

use moor_core::BoxId;
use moor_schema::tables::BoxRefs;
use sea_query::{Expr, ExprTrait as _, OnConflict, Order, Query};

use super::super::{Store, StoreError};
use crate::store::db::DbExt as _;

impl Store {
    /// Record a `kind` ref with `value` on Box `id`; true when it was new.
    ///
    /// # Errors
    /// Returns [`StoreError`] when `kind` is unknown or the database fails.
    #[cfg_attr(not(test), expect(dead_code, reason = "no caller outside tests yet"))]
    pub(crate) fn add_box_ref(
        &self,
        id: BoxId,
        kind: &str,
        value: &str,
    ) -> Result<bool, StoreError> {
        // ON CONFLICT, not OR IGNORE: OR IGNORE would also swallow the kind CHECK.
        let changed = self.writer().run(
            Query::insert()
                .into_table(BoxRefs::Table)
                .columns([BoxRefs::Box, BoxRefs::Kind, BoxRefs::Value])
                .values_panic([id.get().into(), kind.into(), value.into()])
                .on_conflict(OnConflict::new().do_nothing().to_owned()),
        )?;
        Ok(changed > 0)
    }

    /// Values of Box `id`'s refs of `kind`, in the order they were added.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the database fails.
    #[cfg_attr(not(test), expect(dead_code, reason = "no caller outside tests yet"))]
    pub(crate) fn box_refs(&self, id: BoxId, kind: &str) -> Result<Vec<String>, StoreError> {
        let conn = self.reader()?;
        let q = Query::select()
            .column(BoxRefs::Value)
            .from(BoxRefs::Table)
            .and_where(Expr::col(BoxRefs::Box).eq(id.get()))
            .and_where(Expr::col(BoxRefs::Kind).eq(kind))
            .order_by(BoxRefs::Id, Order::Asc)
            .take();
        Ok(conn.rows(q, |row| row.get(0))?.collect::<Result<_, _>>()?)
    }

    /// Boxes carrying a `kind` ref with `value`, ordered by id.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the database fails.
    #[cfg_attr(not(test), expect(dead_code, reason = "no caller outside tests yet"))]
    pub(crate) fn boxes_by_ref(&self, kind: &str, value: &str) -> Result<Vec<BoxId>, StoreError> {
        let conn = self.reader()?;
        let q = Query::select()
            .column(BoxRefs::Box)
            .from(BoxRefs::Table)
            .and_where(Expr::col(BoxRefs::Kind).eq(kind))
            .and_where(Expr::col(BoxRefs::Value).eq(value))
            .order_by(BoxRefs::Box, Order::Asc)
            .take();
        let rows = conn.rows(q, |row| Ok::<_, StoreError>(BoxId::new(row.get(0)?)))?;
        rows.collect()
    }
}

#[cfg(test)]
mod tests;
