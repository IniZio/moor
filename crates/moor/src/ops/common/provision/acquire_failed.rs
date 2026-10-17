//! A failed acquire: record it, then release whatever Box the attempt may have made.

use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, IntentChange};
use moor_core::domain::task::TaskInput;
use moor_core::seam::driver::{AcquireReq, DriverError, Released};
use moor_core::{BoxHandle, TaskId, Warning};

use super::{reread, Entry};
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::release::release_box;
use crate::ops::common::view::task_view;
use crate::store::Store;

/// Records a failed acquire; returns the error, carrying the Task row, to report.
pub(super) fn failed(
    cx: &Cx,
    store: &Store,
    id: TaskId,
    entry: Entry,
    req: &AcquireReq,
    err: DriverError,
) -> CommandError {
    let cause = err.to_string();
    let attempted: BoxHandle = cx.driver().planned_handle(req);
    let mut lost = Decision::new(id);
    lost.task_input = entry.begin().map(|_| TaskInput::AcquireFailed {
        cause: cause.clone(),
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
    // No repo teardown is trusted on a failed start, so `.moor.yaml` is not run.
    let released =
        reread(store, id).and_then(|row| release_box(cx, store, &row, &attempted, false));
    match released {
        Ok(report) if report.driver == Ok(Released::AlreadyGone) => {
            let gone = Decision::new(id).box_input(BoxInput::DriverMissing);
            if let Err(write) = store.apply(gone) {
                return write.into();
            }
        }
        Ok(_) => {}
        Err(e) => cx.warn(Warning {
            code: "box_release_failed".to_owned(),
            message: format!("could not release the failed Box: {}", e.error),
        }),
    }
    let out = CommandError::from(err);
    match reread(store, id) {
        Ok(row) => out.with_value(serde_json::json!({ "task": task_view(&row, cx.config()) })),
        Err(_) => out,
    }
}
