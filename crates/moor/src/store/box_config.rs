//! A Box's config override: partial repo config text laid over its repo's, for that Box only.

use moor_core::BoxId;
use moor_schema::tables::Boxes;
use sea_query::{Expr, ExprTrait as _, Query};

use crate::store::db::{DbExt as _, OptionalExtension};
use crate::store::{Store, StoreError};

impl Store {
    /// Box `id`'s override text; `None` when it has none.
    pub(crate) fn box_config_override(&self, id: BoxId) -> Result<Option<String>, StoreError> {
        let text: Option<Option<String>> = self
            .reader()?
            .row(
                Query::select()
                    .column(Boxes::ConfigOverride)
                    .from(Boxes::Table)
                    .and_where(Expr::col(Boxes::Id).eq(id.get())),
                |r| r.get(0),
            )
            .optional()?;
        Ok(text.flatten())
    }

    /// Replaces Box `id`'s override with `text`, or removes it with `None`.
    pub(crate) fn set_box_config_override(
        &self,
        id: BoxId,
        text: Option<&str>,
    ) -> Result<(), StoreError> {
        self.writer().run(
            Query::update()
                .table(Boxes::Table)
                .value(Boxes::ConfigOverride, text)
                .and_where(Expr::col(Boxes::Id).eq(id.get())),
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
