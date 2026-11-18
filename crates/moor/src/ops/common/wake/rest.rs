//! Putting back to sleep a machine that [`super::wake_to_look`] woke under an asleep Box.

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::decision::{Decision, Wanted};
use moor_core::domain::hands::{HandsInput, HandsState};
use moor_core::seam::driver::SleepOutcome;
use moor_core::{BoxId, Warning};

use crate::cx::owners::Cmd;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::autosleep::settled;
use crate::ops::common::park::{park_tunnel, restore_tunnel};
use crate::ops::common::suspend_check;
use crate::ops::common::wanted::parking;
use crate::store::Store;

/// Sleeps the machine of Box `id` again once nothing runs on it, while the Box itself is still
/// Asleep. A Box a steer or a wake has woken since keeps its machine. A machine that will not
/// sleep stays up with a warning; the next wake of the Box takes it as it is.
pub(crate) fn rest_machine(cx: &Cx, store: &Store, id: BoxId) {
    if let Err(e) = rest(cx, store, id) {
        cx.warn(Warning {
            code: "machine_sleep_failed".to_owned(),
            message: format!(
                "cannot put the machine of Box {} back to sleep: {}",
                id.get(),
                e.error
            ),
        });
    }
}

/// [`rest_machine`], failing on what stops the sleep.
fn rest(cx: &Cx, store: &Store, id: BoxId) -> Result<(), CommandError> {
    let _turn = cx.turn(id, Cmd::Sleep)?;
    let Some(row) = store.box_row(id)? else {
        return Ok(());
    };
    let Some(handle) = row
        .hands
        .as_ref()
        .filter(|b| b.state == HandsState::Ready)
        .and_then(|b| b.handle.clone())
    else {
        return Ok(());
    };
    if row.lifecycle != BoxLifecycle::Asleep
        || cx.owners().busy(id)
        || store.in_flight_run(id)?.is_some()
    {
        return Ok(());
    }
    let inc = row.hands_inc();
    let sleep = || {
        if !suspend_check::record(cx, store, (id, inc), settled(cx, store, &row)?)? {
            return Ok(());
        }
        park_tunnel(cx, id)?;
        match cx.driver().sleep(&handle) {
            Ok(SleepOutcome::Slept | SleepOutcome::Native) => {
                store.apply(Decision::new(id).hands_input(inc, HandsInput::Sleep))?;
                Ok(())
            }
            Ok(SleepOutcome::CannotSleep(reason)) => {
                restore_tunnel(cx, id)?;
                Err(CommandError::new(moor_core::ErrorKind::Refused, reason))
            }
            Err(e) => {
                restore_tunnel(cx, id)?;
                Err(e.into())
            }
        }
    };
    parking(store, id, Wanted::Sleep, sleep).map(drop)
}
