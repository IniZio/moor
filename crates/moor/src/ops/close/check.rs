//! What a close checks a Box for: work that exists only there.

use moor_core::decide::close::{needs_box_check, BoxCheck};
use moor_core::domain::hands::HandsState;
use moor_core::seam::driver::DriverError;
use moor_core::ErrorKind;

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::boxwork::{probe, BoxWork};
use crate::ops::common::wake::wake_to_close;
use crate::store::{BoxRow, Store};

/// Whether a check may wake an asleep Box to read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Wake {
    /// When the Box fits now.
    IfRoom,
    /// Never: an asleep Box is answered from the check stored when it went to sleep.
    Never,
}

/// Looks for work that exists only in the Box, waking a sleeping Box to read it when it fits.
///
/// `row` is reloaded when the wake changed it, so teardown sees a Ready Box. An asleep Box whose
/// check before it was suspended still stands is answered from that when it held work, or when
/// it held none and does not fit to wake. Any other Box that cannot be woken or read gives
/// [`BoxCheck::Unknown`], which the gate refuses.
pub(super) fn box_check(
    cx: &Cx,
    store: &Store,
    row: &mut BoxRow,
    (force, wake): (bool, Wake),
) -> Result<BoxCheck, CommandError> {
    // --force discards Box work, so the Box is neither woken nor probed.
    if force || !needs_box_check(row.lifecycle, row.session) {
        return Ok(BoxCheck::Skipped);
    }
    let live = |r: &BoxRow| {
        r.hands.as_ref().is_some_and(|b| {
            b.handle.is_some() && !matches!(b.state, HandsState::Gone | HandsState::Deleted)
        })
    };
    if !live(row) {
        return Ok(BoxCheck::Skipped);
    }
    let asleep = row
        .hands
        .as_ref()
        .is_some_and(|b| b.state == HandsState::Asleep);
    let stored = if asleep {
        store.suspend_check(row.id)?
    } else {
        None
    };
    if let Some(c) = stored.filter(|c| c.ahead > 0 || c.dirty > 0) {
        return Ok(BoxCheck::Unsaved {
            ahead: c.ahead,
            dirty: c.dirty,
        });
    }
    if asleep && wake == Wake::Never {
        return Ok(stored.map_or_else(
            || BoxCheck::Unknown("its work was not checked when it went to sleep".to_owned()),
            |_| BoxCheck::Clean,
        ));
    }
    // The tunnel stays parked: close needs the Box awake, not reachable through the Proxy.
    match wake_to_close(cx, store, row) {
        Ok(true) => {
            if let Some(fresh) = store.box_row(row.id)? {
                *row = fresh;
            }
        }
        Ok(false) => {}
        // The driver has forgotten the Box: nothing to save, and release treats it as deleted.
        Err(e) if e.error.downcast_ref::<DriverError>() == Some(&DriverError::NotFound) => {
            return Ok(BoxCheck::Skipped);
        }
        Err(e) if e.kind == ErrorKind::Refused && stored.is_some() => return Ok(BoxCheck::Clean),
        Err(e) => {
            return Ok(BoxCheck::Unknown(format!(
                "could not wake the Box: {}",
                e.error
            )));
        }
    }
    // A Box that never got ready, or whose release already failed, has no reachable work tree.
    let Some(b) = row.hands.as_ref().filter(|b| b.state == HandsState::Ready) else {
        return Ok(BoxCheck::Skipped);
    };
    Ok(match probe(cx, row, b) {
        BoxWork::Clean | BoxWork::NoCheckout => BoxCheck::Clean,
        BoxWork::Unsaved { ahead, dirty } => BoxCheck::Unsaved { ahead, dirty },
        BoxWork::Gone => BoxCheck::Skipped,
        BoxWork::Unknown(why) => BoxCheck::Unknown(why),
    })
}
