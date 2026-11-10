//! A close moor still wants: recorded before the close works on its Box, kept when only room to
//! wake the Box stopped it, and finished by a pass once it can be.

use std::cell::Cell;

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::decision::{Decision, ExtraEvent, WantChange, Wanted, WantedKind};
use moor_core::response::{CloseRefusal, ErrorDetail};
use moor_core::view::CloseView;
use moor_core::{BoxId, BoxName, BoxReq, BoxTarget, CloseReq, ErrorKind, Warning};

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
/// One that still has no room to wake is left for the next pass. Any other failure gives the close
/// up with a warning and a `close_refused` event, and a Box the close woke is put back to sleep.
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
        let asleep = row.lifecycle == BoxLifecycle::Asleep;
        let Err(e) = cx.for_box(row.user, || super::run_with_reason(cx, &req, reason)) else {
            continue;
        };
        if e.kind == ErrorKind::Refused && still_wanted(store, row.id)? {
            continue;
        }
        cx.warn(given_up(&row.name, &e.error));
        let mut slept = false;
        if e.kind == ErrorKind::Refused && asleep {
            let req = BoxReq {
                box_name: row.name.clone(),
            };
            match cx.for_box(row.user, || crate::ops::sleep::run(cx, &req)) {
                Ok(_) => slept = true,
                Err(e) => cx.warn(Warning {
                    code: "close_failed".to_owned(),
                    message: format!(
                        "{} stays awake after the close that woke it: {}",
                        row.name.as_str(),
                        e.error
                    ),
                }),
            }
        }
        store.apply(Decision::new(row.id).event(refused(&e, slept)))?;
    }
    Ok(())
}

/// The event that the pass gave up a close for `e`, and whether it put the Box back to sleep.
fn refused(e: &CommandError, slept: bool) -> ExtraEvent {
    let reason = match e.detail.as_deref() {
        Some(ErrorDetail::CloseRefused(r)) => Some(r.clone()),
        _ => None,
    };
    ExtraEvent::CloseRefused {
        reason,
        error: e.error.to_string(),
        slept,
    }
}

/// Whether Box `id` still wants a close, as one refused only for want of room does.
fn still_wanted(store: &Store, id: BoxId) -> Result<bool, CommandError> {
    let row = store.box_row(id)?;
    Ok(row.and_then(|r| r.wanted).map(|w| w.kind()) == Some(WantedKind::Close))
}

/// The warning that the close of `name` was given up, and why.
fn given_up(name: &BoxName, why: &impl std::fmt::Display) -> Warning {
    Warning {
        code: "close_failed".to_owned(),
        message: format!("could not finish the close of {}: {why}", name.as_str()),
    }
}
