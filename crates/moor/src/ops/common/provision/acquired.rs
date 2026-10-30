//! Recording a freshly acquired Box.

use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, Field, Fields, Intent, IntentChange};
use moor_core::seam::driver::Acquired;
use moor_core::BoxId;

use crate::error::CommandError;
use crate::store::Store;

/// Records an acquired Box under the setup intent.
pub(super) fn record_acquired(
    store: &Store,
    id: BoxId,
    got: &Acquired,
) -> Result<(), CommandError> {
    let acquired = Decision::new(id)
        .box_input(BoxInput::Acquired {
            handle: got.handle.clone(),
        })
        .set(Fields {
            box_handle: Field::Set(got.handle.clone()),
            box_workdir: Field::Set(got.workdir.as_str().to_owned()),
            ..Fields::default()
        })
        .intent(IntentChange::Set(Intent::Setup {
            pid: std::process::id(),
        }));
    store.apply(acquired)?;
    Ok(())
}
