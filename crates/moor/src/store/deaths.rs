//! When the places of a Box last died, kept on its row so a Box that keeps dying can be told from
//! one that died once.

use moor_core::BoxId;
use moor_schema::tables::BoxState;
use sea_query::{Expr, ExprTrait as _, Query, Value};

use super::db::{DbExt as _, OptionalExtension};
use super::select::state_update;
use super::{Store, StoreError};

impl Store {
    /// When the places of Box `id` died, in unix milliseconds, oldest first, as far back as
    /// [`moor_core::domain::deaths`] keeps them.
    pub(crate) fn recent_deaths(&self, id: BoxId) -> Result<Vec<i64>, StoreError> {
        let text: Option<Option<String>> = self
            .reader()?
            .row(
                Query::select()
                    .column(BoxState::RecentDeaths)
                    .from(BoxState::Table)
                    .and_where(Expr::col(BoxState::Box).eq(id.get())),
                |r| r.get(0),
            )
            .optional()?;
        Ok(text
            .flatten()
            .map(|t| serde_json::from_str(&t))
            .transpose()?
            .unwrap_or_default())
    }

    /// Records `deaths` as the deaths of Box `id`; none forgets them.
    pub(crate) fn set_recent_deaths(&self, id: BoxId, deaths: &[i64]) -> Result<(), StoreError> {
        let text = if deaths.is_empty() {
            Value::String(None)
        } else {
            serde_json::to_string(deaths)?.into()
        };
        self.writer()
            .run(state_update(id).value(BoxState::RecentDeaths, text))?;
        Ok(())
    }
}
