//! A Box's hands, the VM it runs on: its `box_hands` row with the highest `inc`.

use moor_core::domain::hands::HandsState;
use moor_core::seam::clock::Ts;
use moor_core::shape::Shape;
use moor_core::{BoxHandle, BoxId, ProviderId};
use moor_schema::tables::{BoxHands, Boxes};
use sea_query::{Expr, ExprTrait as _, InsertStatement, Query, SelectStatement, UpdateStatement};

use super::init::parsed_opt;
use super::select::hands_at;
use super::{to_u32, HandsRow, StoreError};
use crate::store::db::{self, Row};

/// The highest `inc` among the hands of the Box `of` names.
fn current_inc(of: Expr) -> SelectStatement {
    Query::select()
        .expr(Expr::col(("current", BoxHands::Inc)).max())
        .from_as(BoxHands::Table, "current")
        .and_where(Expr::col(("current", BoxHands::Box)).eq(of))
        .take()
}

/// `q`, a select from `boxes`, with each Box's current hands joined; a Box without hands gets
/// nulls.
pub(super) fn join_current(q: &mut SelectStatement) -> &mut SelectStatement {
    let h = |c| (BoxHands::Table, c);
    let id = || Expr::col((Boxes::Table, Boxes::Id));
    q.left_join(
        BoxHands::Table,
        Expr::col(h(BoxHands::Box))
            .eq(id())
            .and(Expr::col(h(BoxHands::Inc)).eq(db::sub(current_inc(id())))),
    )
}

/// An UPDATE of the current hands of Box `id`; it changes nothing for a Box without hands.
pub(super) fn update_current(id: BoxId) -> UpdateStatement {
    Query::update()
        .table(BoxHands::Table)
        .and_where(Expr::col(BoxHands::Box).eq(id.get()))
        .and_where(Expr::col(BoxHands::Inc).eq(db::sub(current_inc(Expr::val(id.get())))))
        .take()
}

/// An INSERT of Box `id`'s first hands, in `state` on `provider`.
pub(super) fn first_hands(
    id: BoxId,
    provider: &ProviderId,
    state: HandsState,
    now: Ts,
) -> InsertStatement {
    Query::insert()
        .into_table(BoxHands::Table)
        .columns([
            BoxHands::Box,
            BoxHands::Inc,
            BoxHands::Provider,
            BoxHands::State,
            BoxHands::StateAt,
            BoxHands::CreatedAt,
        ])
        .values_panic([
            id.get().into(),
            1_i64.into(),
            provider.as_str().into(),
            state.as_str().into(),
            now.as_millis().into(),
            now.as_millis().into(),
        ])
        .take()
}

/// A select of `cols` from the current hands of Box `id`; no row for a Box without hands.
pub(super) fn select_current<const N: usize>(id: BoxId, cols: [BoxHands; N]) -> SelectStatement {
    Query::select()
        .columns(cols)
        .from(BoxHands::Table)
        .and_where(Expr::col(BoxHands::Box).eq(id.get()))
        .and_where(Expr::col(BoxHands::Inc).eq(db::sub(current_inc(Expr::val(id.get())))))
        .take()
}

/// The delivered CPUs and memory of the hands in a Box read, when both are recorded.
fn delivered_from_row(row: &Row<'_>) -> Result<Option<Shape>, StoreError> {
    let cpus: Option<i64> = row.get(const { hands_at(BoxHands::Cpus) })?;
    let mem: Option<i64> = row.get(const { hands_at(BoxHands::MemMib) })?;
    let (Some(cpus), Some(mem)) = (cpus, mem) else {
        return Ok(None);
    };
    Ok(Some(Shape {
        cpus: to_u32("cpus", cpus)?,
        mem_mib: u64::try_from(mem)
            .map_err(|_| StoreError::Corrupt(format!("mem_mib {mem} out of range")))?,
    }))
}

/// The hands in a Box read; none for a Box that has none.
pub(super) fn hands_from_row(row: &Row<'_>) -> Result<Option<HandsRow>, StoreError> {
    let Some(inc) = row.get::<_, Option<i64>>(const { hands_at(BoxHands::Inc) })? else {
        return Ok(None);
    };
    let provider: String = row.get(const { hands_at(BoxHands::Provider) })?;
    let state: String = row.get(const { hands_at(BoxHands::State) })?;
    let handle = |at| Ok::<_, StoreError>(row.get::<_, Option<String>>(at)?.map(BoxHandle::new));
    Ok(Some(HandsRow {
        inc,
        provider: ProviderId::new(provider),
        handle: handle(const { hands_at(BoxHands::Handle) })?,
        state: parsed_opt("box state", &state, HandsState::parse)?,
        attempted_handle: handle(const { hands_at(BoxHands::AttemptedHandle) })?,
        workdir: row.get(const { hands_at(BoxHands::Workdir) })?,
        egress: row.get(const { hands_at(BoxHands::Egress) })?,
        sleep_reason: row.get(const { hands_at(BoxHands::SleepReason) })?,
        delivered: delivered_from_row(row)?,
        updated_at: Ts::from_millis(row.get(const { hands_at(BoxHands::StateAt) })?),
    }))
}
