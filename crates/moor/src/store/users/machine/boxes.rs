//! The runner's record of each Box it set up on a machine, kept here so any runner reads it.

use moor_core::name::BoxId;
use moor_core::request::{BoxStage, MachineBox};
use moor_schema::tables::MachineBoxes;
use sea_query::{Expr, ExprTrait as _, OnConflict, Order, Query, SelectStatement};

use crate::store::db::{self, DbExt as _};
use crate::store::{Store, StoreError};

impl Store {
    /// Box `id`'s record, if the runner keeps one.
    pub(crate) fn machine_box(&self, id: BoxId) -> Result<Option<MachineBox>, StoreError> {
        let conn = self.reader()?;
        let mut rows = conn.rows(
            select().and_where(Expr::col(MachineBoxes::BoxId).eq(id.get())),
            box_row,
        )?;
        rows.next().transpose()
    }

    /// Every record on machine `machine_id`, by Box id.
    pub(crate) fn machine_boxes(&self, machine_id: i64) -> Result<Vec<MachineBox>, StoreError> {
        let conn = self.reader()?;
        let rows = conn.rows(
            select()
                .and_where(Expr::col(MachineBoxes::MachineId).eq(machine_id))
                .order_by(MachineBoxes::BoxId, Order::Asc),
            box_row,
        )?;
        rows.collect()
    }

    /// Writes `record` whole over any record of its Box.
    pub(crate) fn put_machine_box(&self, record: &MachineBox) -> Result<(), StoreError> {
        let (prepared, suspended_at) = match record.stage {
            BoxStage::Noted => (false, None),
            BoxStage::Prepared => (true, None),
            BoxStage::Suspended { at } => (true, Some(i64::try_from(at).unwrap_or(i64::MAX))),
        };
        let columns = [
            MachineBoxes::MachineId,
            MachineBoxes::Slot,
            MachineBoxes::Reach,
            MachineBoxes::Facts,
            MachineBoxes::Prepared,
            MachineBoxes::SuspendedAt,
            MachineBoxes::Evicted,
            MachineBoxes::UpdatedAt,
        ];
        self.writer().run(
            Query::insert()
                .into_table(MachineBoxes::Table)
                .columns(std::iter::once(MachineBoxes::BoxId).chain(columns))
                .values_panic([
                    record.box_id.get().into(),
                    record.machine_id.into(),
                    record.slot.into(),
                    record.reach.as_str().into(),
                    record.facts.as_str().into(),
                    prepared.into(),
                    suspended_at.into(),
                    record.evicted.into(),
                    self.now().as_millis().into(),
                ])
                .on_conflict(
                    OnConflict::column(MachineBoxes::BoxId)
                        .update_columns(columns)
                        .to_owned(),
                ),
        )?;
        Ok(())
    }

    /// Removes Box `id`'s record; none is fine.
    pub(crate) fn drop_machine_box(&self, id: BoxId) -> Result<(), StoreError> {
        self.writer().run(
            Query::delete()
                .from_table(MachineBoxes::Table)
                .and_where(Expr::col(MachineBoxes::BoxId).eq(id.get())),
        )?;
        Ok(())
    }
}

/// The select of every record column, in [`box_row`]'s order.
fn select() -> SelectStatement {
    Query::select()
        .columns([
            MachineBoxes::BoxId,
            MachineBoxes::MachineId,
            MachineBoxes::Slot,
            MachineBoxes::Reach,
            MachineBoxes::Facts,
            MachineBoxes::Prepared,
            MachineBoxes::SuspendedAt,
            MachineBoxes::Evicted,
        ])
        .from(MachineBoxes::Table)
        .take()
}

/// A record from a row of [`select`]; a stored state no record can hold reads as corrupt.
fn box_row(r: &db::Row<'_>) -> Result<MachineBox, StoreError> {
    let (prepared, suspended_at): (bool, Option<i64>) = (r.get(5)?, r.get(6)?);
    let slot: i64 = r.get(2)?;
    let stage = match (prepared, suspended_at.map(u64::try_from)) {
        (false, None) => BoxStage::Noted,
        (true, None) => BoxStage::Prepared,
        (true, Some(Ok(at))) => BoxStage::Suspended { at },
        _ => return Err(corrupt("a suspend without a prepare")),
    };
    let Ok(slot) = u16::try_from(slot) else {
        return Err(corrupt("a slot out of range"));
    };
    Ok(MachineBox {
        box_id: BoxId::new(r.get(0)?),
        machine_id: r.get(1)?,
        slot,
        reach: r.get(3)?,
        facts: r.get(4)?,
        stage,
        evicted: r.get(7)?,
    })
}

/// The error for a machine Box record holding `what`.
fn corrupt(what: &str) -> StoreError {
    StoreError::Corrupt(format!("machine box record: {what}"))
}
