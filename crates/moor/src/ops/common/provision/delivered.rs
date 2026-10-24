//! Recording the CPUs and memory a freshly provisioned Box delivers.

use moor_core::domain::decision::{Decision, Field, Fields};
use moor_core::shape::Shape;
use moor_core::BoxId;

use crate::error::CommandError;
use crate::store::Store;

/// Records the CPUs and memory the Box delivers.
pub(super) fn record_delivered(
    store: &Store,
    id: BoxId,
    delivered: Option<Shape>,
) -> Result<(), CommandError> {
    let Some(got) = delivered else {
        return Ok(());
    };
    store.apply(Decision::new(id).set(Fields {
        box_cpus: Field::Set(i64::from(got.cpus)),
        box_mem_mib: Field::Set(i64::try_from(got.mem_mib).unwrap_or(i64::MAX)),
        ..Fields::default()
    }))?;
    Ok(())
}
