//! `sleep`: park a Task's Box.
//!
//! Holds the Box lock for the call. Works from Idle, or from Working when no run is in flight.

use std::collections::HashMap;

use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::boxes::BoxInput;
use moor_core::domain::decision::{Decision, Field, Fields};
use moor_core::seam::driver::SleepOutcome;
use moor_core::view::sleep::SleepValue;
use moor_core::{BoxReq, ErrorKind};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::autosleep::{refuse, sleep_handle, sleep_parked, task_dir};
use crate::ops::common::boxlock::{self, BoxLock};
use crate::ops::common::view::{hide_unsupported_box_sleep, task_view};
use crate::store::BoxRow;

/// Run `sleep`.
pub(crate) fn run(cx: &Cx, req: &BoxReq) -> Result<SleepValue, CommandError> {
    let store = cx.store()?;
    let id = store
        .open_box_by_name(&req.box_name)?
        .ok_or_else(|| {
            CommandError::new(
                ErrorKind::Failed,
                format!("no open task {}", req.box_name.as_str()),
            )
        })?
        .id;
    let _held = BoxLock::acquire(cx.clock(), cx.state_dir()?, id, "sleep", boxlock::WAIT)?;
    let reload = || {
        store.box_row(id)?.ok_or_else(|| {
            CommandError::new(
                ErrorKind::Failed,
                format!("no task {}", req.box_name.as_str()),
            )
        })
    };
    let row = reload()?;
    if row.lifecycle != BoxLifecycle::Asleep {
        sleep_box(cx, &row)?;
    }
    let mut boxed = task_view(&reload()?, cx.config()).driver;
    hide_unsupported_box_sleep(boxed.as_mut(), cx, &mut HashMap::new());
    Ok(SleepValue {
        box_name: req.box_name.as_str().to_owned(),
        driver: boxed,
        woke: false,
    })
}

/// Ask the Driver to sleep the Box and record the outcome.
fn sleep_box(cx: &Cx, row: &BoxRow) -> Result<(), CommandError> {
    let store = cx.store()?;
    if row.lifecycle != BoxLifecycle::Ready {
        return Err(refuse(format!("the Box is {}", row.lifecycle.as_str())));
    }
    let handle = sleep_handle(cx, row)?;
    match sleep_parked(cx, store, row.id, &handle, task_dir(row))? {
        SleepOutcome::Slept | SleepOutcome::Native => {
            let fields = Fields {
                box_sleep_reason: Field::Clear,
                ..Fields::default()
            };
            store.apply(
                Decision::new(row.id)
                    .lifecycle(BoxLifecycleInput::Sleep { auto: false })
                    .box_input(BoxInput::Sleep)
                    .set(fields),
            )?;
            Ok(())
        }
        SleepOutcome::CannotSleep(reason) => {
            let fields = Fields {
                box_sleep_reason: Field::Set(reason.clone()),
                ..Fields::default()
            };
            store.apply(Decision::new(row.id).set(fields))?;
            Err(refuse(format!("the Box cannot sleep: {reason}")))
        }
    }
}

#[cfg(test)]
mod tests;
