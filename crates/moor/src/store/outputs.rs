//! What each Box's agent made outside moor, one row per Box and URL.

use moor_core::output::{Output, PER_BOX_MAX};
use moor_core::seam::clock::Ts;
use moor_core::view::output::OutputView;
use moor_core::BoxId;
use moor_schema::tables::BoxOutputs;
use sea_query::{Expr, ExprTrait as _, OnConflict, Order, Query};

use super::db::DbExt as _;
use super::{Store, StoreError};

/// Whether a report was kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kept {
    /// A new output.
    Added,
    /// An output already reported at that URL, updated in place.
    Updated,
    /// Refused: the Box already has [`PER_BOX_MAX`] outputs.
    Full,
}

impl Store {
    /// Records `output` for Box `id` at `now`; a URL reported again updates its row.
    pub(crate) fn put_output(
        &self,
        id: BoxId,
        output: &Output,
        now: Ts,
    ) -> Result<Kept, StoreError> {
        let known = self.outputs_of(id)?;
        let again = known.iter().any(|o| o.url == output.url());
        if !again && known.len() >= PER_BOX_MAX {
            return Ok(Kept::Full);
        }
        let at = now.as_millis();
        self.writer().run(
            Query::insert()
                .into_table(BoxOutputs::Table)
                .columns([
                    BoxOutputs::Box,
                    BoxOutputs::Url,
                    BoxOutputs::Kind,
                    BoxOutputs::Title,
                    BoxOutputs::FirstAt,
                    BoxOutputs::LastAt,
                ])
                .values_panic([
                    id.get().into(),
                    output.url().into(),
                    output.kind().as_str().into(),
                    output.title().into(),
                    at.into(),
                    at.into(),
                ])
                .on_conflict(
                    OnConflict::columns([BoxOutputs::Box, BoxOutputs::Url])
                        .update_columns([BoxOutputs::Kind, BoxOutputs::Title, BoxOutputs::LastAt])
                        .to_owned(),
                ),
        )?;
        Ok(if again { Kept::Updated } else { Kept::Added })
    }

    /// The outputs of Box `id`, oldest first.
    pub(crate) fn outputs_of(&self, id: BoxId) -> Result<Vec<OutputView>, StoreError> {
        self.reader()?
            .rows(
                Query::select()
                    .columns([
                        BoxOutputs::Kind,
                        BoxOutputs::Url,
                        BoxOutputs::Title,
                        BoxOutputs::FirstAt,
                        BoxOutputs::LastAt,
                    ])
                    .from(BoxOutputs::Table)
                    .and_where(Expr::col(BoxOutputs::Box).eq(id.get()))
                    .order_by(BoxOutputs::FirstAt, Order::Asc)
                    .order_by(BoxOutputs::Url, Order::Asc),
                |r| {
                    Ok::<_, StoreError>(OutputView {
                        kind: r.get(0)?,
                        url: r.get(1)?,
                        title: r.get(2)?,
                        first_at: r.get(3)?,
                        last_at: r.get(4)?,
                    })
                },
            )?
            .collect()
    }
}
