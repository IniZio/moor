//! A Box's hands, the VM it runs on: its `box_hands` row with the highest `inc`.

use moor_core::domain::decision::Decision;
use moor_core::domain::hands::{HandsInc, HandsInput, HandsState};
use moor_core::seam::clock::Ts;
use moor_core::shape::Shape;
use moor_core::{BoxHandle, BoxId, ProviderId};
use moor_schema::tables::{BoxHands, BoxLease, Boxes};
use sea_query::{
    Cond, Condition, Expr, ExprTrait as _, InsertStatement, Order, Query, SelectStatement,
    UpdateStatement,
};

use super::apply::apply_in;
use super::init::parsed_opt;
use super::lease::{LeaseId, LeaseState};
use super::select::hands_at;
use super::Fence;
use super::{to_u32, BoxRow, HandsRow, Store, StoreError};
use crate::store::db::{self, Db, DbExt as _, Row};

impl Store {
    /// Applies `moves`, each to Box `id`, and holds lease `hold`, in one transaction: a Box moving
    /// to new hands ends the old ones, gets the new ones and holds their machine, or none of it.
    pub(crate) fn move_hands(
        &self,
        id: BoxId,
        moves: Vec<Decision>,
        hold: LeaseId,
    ) -> Result<(), StoreError> {
        self.check_write(id);
        let now = self.now();
        let mut conn = self.writer();
        let tx = conn.immediate()?;
        for d in moves {
            apply_in(&tx, now, d)?;
        }
        tx.run(
            Query::update()
                .table(BoxLease::Table)
                .value(BoxLease::State, LeaseState::Held.as_str())
                .value(BoxLease::UpdatedAt, now.as_millis())
                .and_where(Expr::col(BoxLease::Id).eq(hold.0)),
        )?;
        tx.commit()?;
        drop(conn);
        Ok(())
    }

    /// The hands Box `id` left whose machine no release has ended yet: replaced hands that ended
    /// Gone with a handle, oldest first.
    pub(crate) fn hands_left(&self, id: BoxId) -> Result<Vec<(HandsInc, BoxHandle)>, StoreError> {
        let rows = self.reader()?.rows(
            Query::select()
                .columns([BoxHands::Inc, BoxHands::Handle])
                .from(BoxHands::Table)
                .cond_where(left(id))
                .order_by(BoxHands::Inc, Order::Asc),
            |r| {
                let handle: String = r.get(1)?;
                Ok::<_, db::Error>((HandsInc::new(r.get(0)?), BoxHandle::new(handle)))
            },
        )?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Records that the machine of hands `inc`, which Box `id` left, is released.
    pub(crate) fn left_hands_released(&self, id: BoxId, inc: HandsInc) -> Result<(), StoreError> {
        let now = self.now().as_millis();
        let mut done = Query::update();
        done.table(BoxHands::Table)
            .value(BoxHands::State, HandsState::Deleted.as_str())
            .value(BoxHands::StateAt, now)
            .cond_where(left(id))
            .and_where(Expr::col(BoxHands::Inc).eq(inc.get()));
        self.writer().run(done)?;
        Ok(())
    }
}

/// The hands Box `id` left whose machine no release has ended: not its current ones, Gone, with a
/// handle.
fn left(id: BoxId) -> Condition {
    Cond::all()
        .add(Expr::col(BoxHands::Box).eq(id.get()))
        .add(Expr::col(BoxHands::State).eq(HandsState::Gone.as_str()))
        .add(Expr::col(BoxHands::Handle).is_not_null())
        .add(Expr::col(BoxHands::Inc).lt(db::sub(current_inc(Expr::val(id.get())))))
}

impl BoxRow {
    /// The hands an acquire for this Box writes: new ones after hands that failed or were lost.
    pub(crate) fn acquire_inc(&self) -> HandsInc {
        HandsInc::acquiring(self.hands.as_ref().map(|h| (h.inc, h.state)))
    }
}

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

/// An UPDATE of the hands `inc` of Box `id`, while they are its current ones; it changes nothing
/// once the Box has newer hands.
pub(super) fn update_hands(id: BoxId, inc: HandsInc) -> UpdateStatement {
    Query::update()
        .table(BoxHands::Table)
        .and_where(Expr::col(BoxHands::Box).eq(id.get()))
        .and_where(Expr::col(BoxHands::Inc).eq(inc.get()))
        .and_where(Expr::col(BoxHands::Inc).eq(db::sub(current_inc(Expr::val(id.get())))))
        .take()
}

/// The hands of Box `id` that `d` writes. Those are its `current` ones, or new ones a `Requested`
/// after current hands that ended creates. Any other write is refused: the hands it names were
/// replaced after the op read them, so it is about a machine the Box no longer has.
pub(super) fn fence(
    id: BoxId,
    current: Option<&HandsRow>,
    d: &Decision,
) -> Result<HandsInc, StoreError> {
    let held = current.map(|h| (h.inc, h.state));
    let now = held.map_or(HandsInc::FIRST, |(inc, _)| inc);
    let Some(w) = &d.hands else {
        return Ok(now);
    };
    let acquires = matches!(w.input, Some(HandsInput::Requested));
    if w.inc == now || (acquires && w.inc == HandsInc::acquiring(held)) {
        return Ok(w.inc);
    }
    Err(StoreError::Stale {
        box_id: id,
        read: Fence::Hands(w.inc),
    })
}

/// What an [`update_hands`] that changed `rows` rows comes to: none changed means the hands `inc`
/// are no longer the Box's current ones.
pub(super) const fn fenced(id: BoxId, inc: HandsInc, rows: usize) -> Result<(), StoreError> {
    if rows == 0 {
        return Err(StoreError::Stale {
            box_id: id,
            read: Fence::Hands(inc),
        });
    }
    Ok(())
}

/// Refuses a write about Box `id`'s hands `inc` once the Box has newer hands. A write that names
/// no hands, as an older runner sends, is let through.
pub(super) fn fence_inc(conn: &dyn Db, id: BoxId, inc: Option<HandsInc>) -> Result<(), StoreError> {
    let Some(inc) = inc else {
        return Ok(());
    };
    let current: Option<i64> = conn.row(current_inc(Expr::val(id.get())), |r| r.get(0))?;
    if current.is_some_and(|now| now > inc.get()) {
        return Err(StoreError::Stale {
            box_id: id,
            read: Fence::Hands(inc),
        });
    }
    Ok(())
}

/// An INSERT of Box `id`'s hands `inc`, in `state` on `provider`.
pub(super) fn new_hands(
    (id, inc): (BoxId, HandsInc),
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
            inc.get().into(),
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
    let inc = HandsInc::new(inc);
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
