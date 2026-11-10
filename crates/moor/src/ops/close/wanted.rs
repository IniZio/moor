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
/// up while the turn still holds the Box. Under `tell`, the same write records `close_refused`
/// when the Box is still open. A give-up that cannot be written is warned of and stays wanted.
/// Once `close` lets the turn go, the Box is released and its lifecycle has ended what it wanted.
pub(super) fn closing<'a>(
    cx: &Cx,
    id: BoxId,
    wanted: Wanted,
    (mut turn, tell): (Option<Turn<'a>>, bool),
    close: impl FnOnce(&Cell<bool>, &mut Option<Turn<'a>>) -> Result<CloseView, CommandError>,
) -> Result<CloseView, CommandError> {
    let store = cx.store()?;
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
            let mut done = Decision::new(id).want(WantChange::Done(WantedKind::Close));
            if tell && still_open(store, id) {
                done = done.event(refused(&e));
            }
            if let Err(why) = store.apply(done) {
                cx.warn(Warning {
                    code: "close_failed".to_owned(),
                    message: format!(
                        "could not record that the close of Box {} was given up, so it stays \
                         wanted: {why}",
                        id.get()
                    ),
                });
            }
            Err(e)
        }
        done => done,
    }
}

/// Closes each open Box that wants a close and has no op or effect on it, as the close it wanted.
/// One that still has no room to wake is left for the next pass. Any other failure gives the close
/// up with a warning and, in the same write that drops the want, a `close_refused` event. An
/// asleep Box the close woke is then put back to sleep, which records its own `asleep` event.
/// A store failure on one Box is warned of and the pass goes on to the next.
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
        let Err(e) = cx.for_box(row.user, || super::run_wanted(cx, &req, reason)) else {
            continue;
        };
        if e.kind == ErrorKind::Refused {
            match still_wanted(store, row.id) {
                Ok(false) => {}
                Ok(true) => continue,
                Err(why) => {
                    cx.warn(given_up(&row.name, &why.error));
                    continue;
                }
            }
        }
        cx.warn(given_up(&row.name, &e.error));
        if e.kind == ErrorKind::Refused && asleep {
            let req = BoxReq {
                box_name: row.name.clone(),
            };
            if let Err(e) = cx.for_box(row.user, || crate::ops::sleep::run(cx, &req)) {
                cx.warn(Warning {
                    code: "close_failed".to_owned(),
                    message: format!(
                        "{} stays awake after the close that woke it: {}",
                        row.name.as_str(),
                        e.error
                    ),
                });
            }
        }
    }
    Ok(())
}

/// The event that a close moor still wanted was given up for `e`.
fn refused(e: &CommandError) -> ExtraEvent {
    let reason = match e.detail.as_deref() {
        Some(ErrorDetail::CloseRefused(r)) => Some(r.clone()),
        _ => None,
    };
    ExtraEvent::CloseRefused {
        reason,
        error: e.error.to_string(),
    }
}

/// Whether Box `id` reads as open; a close that ended it, or a read that failed, is not.
fn still_open(store: &Store, id: BoxId) -> bool {
    let row = store.box_row(id).ok().flatten();
    row.is_some_and(|r| r.lifecycle.is_live())
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
