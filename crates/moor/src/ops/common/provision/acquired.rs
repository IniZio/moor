//! Recording a freshly acquired Box.

use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, ExtraEvent, Field, Fields, Intent, IntentChange};
use moor_core::seam::driver::Acquired;
use moor_core::BoxId;

use crate::error::CommandError;
use crate::store::{BoxRow, Store};

/// Records that the machine just replaced took its disk with it when `before` shows it gone, and
/// drops any mark to resume it, since nothing is left to resume.
fn record_disk_lost(store: &Store, before: &BoxRow) -> Result<(), CommandError> {
    let gone = before
        .boxed
        .as_ref()
        .is_some_and(|b| b.state == BoxState::Gone);
    if gone {
        store.apply(Decision::new(before.id).event(ExtraEvent::DiskLost))?;
        store.set_resume_first(before.id, None)?;
    }
    Ok(())
}

/// Records an acquired Box under the setup intent; `replacing` is what it held before, when a
/// working Box lost its machine.
pub(super) fn record_acquired(
    store: &Store,
    id: BoxId,
    got: &Acquired,
    replacing: Option<&BoxRow>,
) -> Result<(), CommandError> {
    if let Some(before) = replacing {
        record_disk_lost(store, before)?;
    }
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
