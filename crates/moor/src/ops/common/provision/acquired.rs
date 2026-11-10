//! Recording a freshly acquired Box.

use moor_core::domain::decision::{Decision, Effect, EffectChange, ExtraEvent, Field, HandsFields};
use moor_core::domain::hands::{HandsInc, HandsInput, HandsState};
use moor_core::seam::driver::Acquired;
use moor_core::BoxId;

use crate::error::CommandError;
use crate::store::{BoxRow, Store};

/// Records that the machine just replaced took its disk with it when `before` shows it gone, and
/// drops any mark to resume it, since nothing is left to resume.
fn record_disk_lost(store: &Store, before: &BoxRow) -> Result<(), CommandError> {
    let gone = before
        .hands
        .as_ref()
        .is_some_and(|b| b.state == HandsState::Gone);
    if gone {
        store.apply(Decision::new(before.id).event(ExtraEvent::DiskLost))?;
        store.set_resume_first(before.id, None)?;
    }
    Ok(())
}

/// Records the machine acquired for hands `inc`, and the setup that follows as the effect in flight; `replacing` is what it held before, when a
/// working Box lost its machine.
pub(super) fn record_acquired(
    store: &Store,
    (id, inc): (BoxId, HandsInc),
    got: &Acquired,
    replacing: Option<&BoxRow>,
) -> Result<(), CommandError> {
    if let Some(before) = replacing {
        record_disk_lost(store, before)?;
    }
    let acquired = Decision::new(id)
        .hands_input(
            inc,
            HandsInput::Acquired {
                handle: got.handle.clone(),
            },
        )
        .hands(
            inc,
            HandsFields {
                handle: Field::Set(got.handle.clone()),
                workdir: Field::Set(got.workdir.as_str().to_owned()),
                ..HandsFields::default()
            },
        )
        .effect(EffectChange::Next(Effect::Setup));
    store.apply(acquired)?;
    Ok(())
}
