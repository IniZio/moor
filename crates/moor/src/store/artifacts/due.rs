//! Durable "collection is due" tracking: a Box is due when a turn ended after its last collection.

use moor_core::BoxId;
use moor_schema::tables::{ArtifactCursors, Boxes, Events};
use sea_query::{Expr, ExprTrait as _, Func, JoinType, OnConflict, Order, Query, SelectStatement};

use crate::store::db::{DbExt as _, OptionalExtension};
use crate::store::{Store, StoreError};

/// Event kinds that end a turn.
const TURN_ENDS: [&str; 2] = ["idled", "turn_done"];

/// The id of the newest turn-ending event, as `newest`, from the events `filter` keeps.
fn newest_turn_event(filter: Option<Expr>) -> SelectStatement {
    Query::select()
        .expr_as(Expr::col(Events::Id).max(), "newest")
        .from(Events::Table)
        .and_where(Expr::col(Events::Kind).is_in(TURN_ENDS))
        .and_where_option(filter)
        .take()
}

/// Ready, handled, intent-free open Boxes.
fn collectable(q: &mut SelectStatement) -> &mut SelectStatement {
    q.and_where(Expr::col((Boxes::Table, Boxes::State)).eq("ready"))
        .and_where(Expr::col((Boxes::Table, Boxes::Handle)).is_not_null())
        .and_where(Expr::col((Boxes::Table, Boxes::IntentKind)).is_null())
        .order_by((Boxes::Table, Boxes::Id), Order::Asc)
}

impl Store {
    /// Ready, handled, intent-free open Boxes whose newest turn-end event is past their
    /// collection cursor, as (Box, that event id), ordered by Box id.
    pub(crate) fn artifacts_due(&self) -> Result<Vec<(BoxId, i64)>, StoreError> {
        let per_box = newest_turn_event(None)
            .column(Events::Box)
            .group_by_col(Events::Box)
            .take();
        let through = Expr::col((ArtifactCursors::Table, ArtifactCursors::Through));
        let mut q = Query::select();
        q.column((Boxes::Table, Boxes::Id))
            .column(("e", "newest"))
            .from(Boxes::Table)
            .join_subquery(
                JoinType::InnerJoin,
                per_box,
                "e",
                Expr::col(("e", Events::Box)).equals((Boxes::Table, Boxes::Id)),
            )
            .left_join(
                ArtifactCursors::Table,
                Expr::col((ArtifactCursors::Table, ArtifactCursors::Box))
                    .equals((Boxes::Table, Boxes::Id)),
            )
            .and_where(Expr::col(("e", "newest")).gt(Func::coalesce([through, Expr::value(0)])));
        let rows = self.reader()?.rows(collectable(&mut q), |r| {
            Ok::<_, StoreError>((BoxId::new(r.get(0)?), r.get::<_, i64>(1)?))
        })?;
        rows.collect()
    }

    /// Ready, handled, intent-free open Boxes, by Box id; a Box mid-turn is among them.
    pub(crate) fn artifacts_ready(&self) -> Result<Vec<BoxId>, StoreError> {
        let mut q = Query::select();
        q.column((Boxes::Table, Boxes::Id)).from(Boxes::Table);
        let rows = self.reader()?.rows(collectable(&mut q), |r| {
            Ok::<_, StoreError>(BoxId::new(r.get(0)?))
        })?;
        rows.collect()
    }

    /// Newest `idled` or `turn_done` event id of the Box, none when it has had neither.
    pub(crate) fn newest_turn_event(&self, id: BoxId) -> Result<Option<i64>, StoreError> {
        let q = newest_turn_event(Some(Expr::col(Events::Box).eq(id.get())));
        let newest: Option<i64> = self.reader()?.row(q, |r| r.get(0)).optional()?.flatten();
        Ok(newest)
    }

    /// Records that artifacts were collected through event `through`; never moves backwards.
    pub(crate) fn set_artifacts_through(&self, id: BoxId, through: i64) -> Result<(), StoreError> {
        let kept = Func::greatest([
            Expr::col((ArtifactCursors::Table, ArtifactCursors::Through)),
            Expr::col(("excluded", ArtifactCursors::Through)),
        ]);
        let conn = self.writer();
        conn.run(
            Query::insert()
                .into_table(ArtifactCursors::Table)
                .columns([ArtifactCursors::Box, ArtifactCursors::Through])
                .values_panic([id.get().into(), through.into()])
                .on_conflict(
                    OnConflict::column(ArtifactCursors::Box)
                        .value(ArtifactCursors::Through, kept)
                        .to_owned(),
                ),
        )?;
        drop(conn);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
