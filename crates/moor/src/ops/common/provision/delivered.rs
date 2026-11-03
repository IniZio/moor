//! Recording the CPUs and memory a freshly provisioned Box delivers.

use moor_core::config::BoxSize;
use moor_core::domain::decision::{Decision, Field, Fields};
use moor_core::shape::Shape;
use moor_core::{BoxId, Warning};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::Store;

/// Records the CPUs and memory the Box delivers, and warns when that is less than `size` asks for.
pub(super) fn record_delivered(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    size: BoxSize,
    delivered: Option<Shape>,
) -> Result<(), CommandError> {
    let Some(got) = delivered else {
        return Ok(());
    };
    if got.mem_mib < size.least_mem_mib() {
        cx.warn(Warning {
            code: "machine_short".to_owned(),
            message: format!(
                "Box {} asked for a {} machine ({} MiB) and got {} MiB",
                id.get(),
                size.as_str(),
                size.mem_mib(),
                got.mem_mib
            ),
        });
    }
    store.apply(Decision::new(id).set(Fields {
        box_cpus: Field::Set(i64::from(got.cpus)),
        box_mem_mib: Field::Set(i64::try_from(got.mem_mib).unwrap_or(i64::MAX)),
        ..Fields::default()
    }))?;
    Ok(())
}
