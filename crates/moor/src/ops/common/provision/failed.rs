//! Recording a failed acquire.

use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, IntentChange};
use moor_core::seam::driver::{AcquireReq, DriverError, Released};
use moor_core::{BoxHandle, BoxId, Warning};

use super::{reread, Entry};
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::view::box_view;
use crate::store::Store;

/// Records a failed acquire; returns the error, carrying the Box row, to report.
pub(super) fn failed(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    entry: Entry,
    req: &AcquireReq,
    err: DriverError,
) -> CommandError {
    let cause = err.to_string();
    let attempted: BoxHandle = cx.driver().planned_handle(req);
    let mut lost = Decision::new(id);
    lost.lifecycle_input = entry.sets_up().then(|| BoxLifecycleInput::SetupFailed {
        cause: cause.clone(),
        which: None,
    });
    let lost = lost
        .box_input(BoxInput::AcquireFailed {
            cause,
            attempted: Some(attempted.clone()),
        })
        .intent(IntentChange::Clear);
    if let Err(write) = store.apply(lost) {
        return write.into();
    }
    match cx.driver().release(&attempted) {
        Ok(Released::AlreadyGone) => {
            let gone = Decision::new(id).box_input(BoxInput::DriverMissing);
            if let Err(write) = store.apply(gone) {
                return write.into();
            }
        }
        Ok(Released::Deleted) => {}
        Err(e) => cx.warn(Warning {
            code: "box_release_failed".to_owned(),
            message: format!(
                "could not release {}: {e}; closing the Box stops it",
                attempted.as_str()
            ),
        }),
    }
    let out = CommandError::from(err);
    match reread(store, id) {
        Ok(row) => out.with_value(serde_json::json!({ "box": box_view(&row, cx.config()) })),
        Err(_) => out,
    }
}
