//! A close moor still wants: recorded before the close works on its Box, kept when only room to
//! wake the Box stopped it, and finished by a pass once it can be.

use std::cell::Cell;

use moor_core::domain::decision::{Decision, WantChange, Wanted, WantedKind};
use moor_core::response::{CloseRefusal, ErrorDetail};
use moor_core::view::CloseView;
use moor_core::{BoxId, BoxTarget, CloseReq, ErrorKind, Warning};

use crate::cx::owners::Turn;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::Store;

/// Runs `close` on Box `id` with `wanted` raised first, so a restart cutting the close finishes
/// it. `turn` is the turn the close took, `None` when its thread already held one. A close refused
/// only because the Box had no room to wake stays wanted and says so; any other failure gives it
/// up while the turn still holds the Box. Once `close` lets the turn go, the Box is released and
/// its lifecycle has ended what it wanted.
pub(super) fn closing<'a>(
    store: &Store,
    id: BoxId,
    wanted: Wanted,
    mut turn: Option<Turn<'a>>,
    close: impl FnOnce(&Cell<bool>, &mut Option<Turn<'a>>) -> Result<CloseView, CommandError>,
) -> Result<CloseView, CommandError> {
    store.apply(Decision::new(id).want(WantChange::Raise(wanted)))?;
    let took = turn.is_some();
    let waits = Cell::new(false);
    match close(&waits, &mut turn) {
        Err(mut e) if waits.get() => {
            e.error = format!("{}; moor closes it once its node has room", e.error).into();
            if let Some(ErrorDetail::CloseRefused(CloseRefusal::Unchecked { reason })) =
                e.detail.as_deref().cloned()
            {
                let room = CloseRefusal::WaitingForRoom { reason };
                e.detail = Some(Box::new(ErrorDetail::CloseRefused(room)));
            }
            Err(e)
        }
        Err(e) if turn.is_some() || !took => {
            store
                .apply(Decision::new(id).want(WantChange::Done(WantedKind::Close)))
                .ok();
            Err(e)
        }
        done => done,
    }
}

/// Closes each open Box that wants a close and has no op or effect on it, as the close it wanted.
/// One that still has no room to wake is left for the next pass, and another failure is warned.
///
/// # Errors
/// A store failure listing the Boxes.
pub(crate) fn pass(
    cx: &Cx,
    store: &Store,
    draining: &dyn Fn() -> bool,
) -> Result<(), CommandError> {
    for row in store.wanting_close()? {
        if draining() {
            break;
        }
        let Some(Wanted::Close { force, reason }) = row.wanted else {
            continue;
        };
        if cx.owners().busy(row.id) {
            continue;
        }
        let req = CloseReq {
            target: BoxTarget::Name(row.name.clone()),
            force,
        };
        match cx.for_box(row.user, || super::run_with_reason(cx, &req, reason)) {
            Err(e) if e.kind != ErrorKind::Refused => cx.warn(Warning {
                code: "close_failed".to_owned(),
                message: format!(
                    "could not finish the close of {}: {}",
                    row.name.as_str(),
                    e.error
                ),
            }),
            _ => {}
        }
    }
    Ok(())
}
