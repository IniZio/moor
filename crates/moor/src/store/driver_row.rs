//! The machine columns of a Box row.

use moor_core::seam::clock::Ts;
use moor_core::shape::Shape;
use moor_core::{BoxHandle, ProviderId};
use rusqlite::Row;

use super::init::parsed_opt;
use super::{to_u32, DriverRow, StoreError};
use moor_core::domain::boxes::BoxState;

/// The delivered CPUs and memory of the joined box, when both are recorded.
pub(super) fn delivered_from_row(row: &Row<'_>) -> Result<Option<Shape>, StoreError> {
    let cpus: Option<i64> = row.get(33)?;
    let mem: Option<i64> = row.get(34)?;
    let (Some(cpus), Some(mem)) = (cpus, mem) else {
        return Ok(None);
    };
    Ok(Some(Shape {
        cpus: to_u32("cpus", cpus)?,
        mem_mib: u64::try_from(mem)
            .map_err(|_| StoreError::Corrupt(format!("mem_mib {mem} out of range")))?,
    }))
}

/// Map the joined box columns (offset 18..=25) to a `DriverRow`, if the box exists.
pub(super) fn driver_from_row(row: &Row<'_>) -> Result<Option<DriverRow>, StoreError> {
    let Some(provider) = row.get::<_, Option<String>>(18)? else {
        return Ok(None);
    };
    let state: String = row.get(20)?;
    Ok(Some(DriverRow {
        provider: ProviderId::new(provider),
        handle: row.get::<_, Option<String>>(19)?.map(BoxHandle::new),
        state: parsed_opt("box state", &state, BoxState::parse)?,
        attempted_handle: row.get::<_, Option<String>>(21)?.map(BoxHandle::new),
        workdir: row.get(22)?,
        egress: row.get(23)?,
        sleep_reason: row.get(24)?,
        delivered: delivered_from_row(row)?,
        updated_at: Ts::from_millis(row.get(25)?),
    }))
}
