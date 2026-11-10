//! Settles the starts an earlier controller process left unfinished.

use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::decision::{Decision, Effect, EffectChange, Restart, Wanted, WantedKind};
use moor_core::domain::hands::{HandsInput, HandsState};
use moor_core::seam::driver::Released;
use moor_core::{BoxHandle, BoxId, Warning};

use super::lease::planned_root;
use super::rerun::{record_interrupted, NOT_READY};
use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::{BoxRow, Store};

/// Why a Box whose acquire was cut off failed.
const CAUSE: &str = "setup interrupted by a controller restart";

/// The open Boxes an earlier controller process left something in flight on: an effect, or a
/// close it wanted. The controller lock keeps every other process off the store, so each is dead
/// and settles as its owner's first turn.
pub(crate) fn cut_short(store: &Store) -> Result<Vec<BoxId>, CommandError> {
    Ok(store.in_flight_boxes()?.into_iter().map(|r| r.id).collect())
}

/// What a resume does once its Box has settled, which is when the controller may serve.
#[derive(Debug)]
pub(crate) enum Then {
    /// Nothing more.
    Rest,
    /// [`super::resume_setup`] goes on with the start.
    Resume,
    /// Release the machine the cut acquire may have made, then acquire again when `again` and the
    /// release went through.
    Release {
        /// The machine.
        handle: BoxHandle,
        /// Whether this start acquires again.
        again: bool,
    },
}

/// Settles what Box `id` had in flight under the Resume turn the caller holds, with no call to a
/// provider. A wanted close goes on first. An effect that is safe to repeat goes on when its
/// setup can be followed again or finished in its machine, its acquire was cut for the first time
/// in this start, its launch was cut before an agent was recorded, or its release was cut. A
/// cut run is reported, as running its command again could repeat what it did.
pub(crate) fn settle(cx: &Cx, store: &Store, id: BoxId) -> Result<Then, CommandError> {
    let Some(row) = store.box_row(id)? else {
        return Ok(Then::Rest);
    };
    if row.wanted.as_ref().map(Wanted::kind) == Some(WantedKind::Close) {
        return Ok(Then::Resume);
    }
    let Some(effect) = row.effect else {
        return Ok(Then::Rest);
    };
    if effect.on_restart() == Restart::Report {
        report_run(store, &row)?;
        return Ok(Then::Rest);
    }
    match effect {
        Effect::Acquire => cx.for_box(row.user, || cut_acquire(cx, store, &row)),
        Effect::Setup if followable(&row) => Ok(Then::Resume),
        Effect::Setup => {
            record_interrupted(cx, store, &row, NOT_READY.to_owned()).map(|()| Then::Rest)
        }
        Effect::Launch if launch_was_cut(store, &row)? => Ok(Then::Resume),
        Effect::Launch | Effect::Run { .. } => {
            store.apply(Decision::new(id).effect(EffectChange::End))?;
            Ok(Then::Rest)
        }
        Effect::Release => Ok(Then::Resume),
    }
}

/// Ends the run a restart cut, as interrupted, when its row is still open, and its effect. A run
/// someone still follows is left with its effect.
fn report_run(store: &Store, row: &BoxRow) -> Result<(), CommandError> {
    if store.in_flight_run(row.id)?.is_some() {
        return Ok(());
    }
    if matches!(
        store.box_row(row.id)?.and_then(|r| r.effect),
        Some(Effect::Run { .. })
    ) {
        store.apply(Decision::new(row.id).effect(EffectChange::End))?;
    }
    Ok(())
}

/// Does what `then` asks for Box `id`, still under the caller's Resume turn; returns whether
/// [`super::resume_setup`] goes on with the start.
pub(crate) fn release_then(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    then: Then,
) -> Result<bool, CommandError> {
    let (handle, again) = match then {
        Then::Rest => return Ok(false),
        Then::Resume => return Ok(true),
        Then::Release { handle, again } => (handle, again),
    };
    let Some(row) = store.box_row(id)? else {
        return Ok(false);
    };
    let released = cx.for_box(row.user, || release_warned(cx, store, &row, &handle));
    if again && !released {
        store.apply(gave_up(&row))?;
    }
    Ok(again && released)
}

/// Settles every Box [`cut_short`] finds, each under a Resume turn of its own; returns those that
/// go on, for [`super::resume_setup`].
#[cfg(test)]
pub(crate) fn fail_interrupted(cx: &Cx, store: &Store) -> Result<Vec<BoxId>, CommandError> {
    let mut claimed = Vec::new();
    for id in cut_short(store)? {
        let _turn = cx.turn(id, crate::cx::owners::Cmd::Resume)?;
        let then = settle(cx, store, id)?;
        if release_then(cx, store, id, then)? {
            claimed.push(id);
        }
    }
    Ok(claimed)
}

/// Whether the launch in flight on `row` was cut before an agent was recorded, on a machine that
/// is still ready. A `--no-agent` start never begins one, and a launch that failed says so.
pub(super) fn launch_was_cut(store: &Store, row: &BoxRow) -> Result<bool, CommandError> {
    let ready = row
        .hands
        .as_ref()
        .is_some_and(|b| b.state == HandsState::Ready);
    let waits = row.lifecycle == BoxLifecycle::Ready && !row.session.is_quiet();
    Ok(row.effect == Some(Effect::Launch)
        && ready
        && waits
        && row.launch_failure.is_none()
        && store.live_launch(row.id)?.is_none())
}

/// Whether the cut-off start of `row` was setting up in a machine it still holds: running its
/// setup lines, or the steps after them once every line passed.
fn followable(row: &BoxRow) -> bool {
    row.lifecycle == BoxLifecycle::SettingUp
        && row.hands.as_ref().is_some_and(|b| {
            b.state == HandsState::Ready && b.handle.is_some() && b.workdir.is_some()
        })
}

/// Settles a cut-off acquire; returns what this process does with it once serving.
fn cut_acquire(cx: &Cx, store: &Store, row: &BoxRow) -> Result<Then, CommandError> {
    let setting_up = row.lifecycle == BoxLifecycle::SettingUp;
    match row.hands.as_ref().map(|b| b.state) {
        Some(HandsState::Starting) => {
            let planned = planned_root(store, row.id)?;
            let goes_on = planned
                .as_ref()
                .is_some_and(|h| cx.driver().acquire_outlives_caller(h));
            if goes_on && setting_up {
                return Ok(Then::Resume);
            }
            if goes_on {
                // The resume stops it once the controller serves, as stopping waits for it.
                let lost = Decision::new(row.id).hands_input(
                    row.hands_inc(),
                    HandsInput::AcquireFailed {
                        cause: CAUSE.to_owned(),
                        attempted: planned,
                    },
                );
                store.apply(lost)?;
                return Ok(Then::Resume);
            }
            fail_acquire(store, row, setting_up && !cut_before(row))
        }
        // The process that claimed it to acquire again ended before it could.
        Some(HandsState::Failed | HandsState::Gone) if setting_up => {
            let lost = Decision::new(row.id)
                .lifecycle(BoxLifecycleInput::SetupFailed {
                    cause: CAUSE.to_owned(),
                    which: None,
                })
                .effect(EffectChange::End);
            store.apply(lost)?;
            Ok(Then::Rest)
        }
        // Hands that came up, or none, leave nothing of the acquire to settle.
        _ => {
            store.apply(Decision::new(row.id).effect(EffectChange::End))?;
            Ok(Then::Rest)
        }
    }
}

/// Whether this start already asked for a machine twice, as a cut acquire that goes again does.
const fn cut_before(row: &BoxRow) -> bool {
    row.effect_attempt >= 2
}

/// Records the cut-off acquire as failed; returns that the machine it may have made is released
/// once serving, then acquired again when `again` asks. Not acquiring again settles the Box now.
fn fail_acquire(store: &Store, row: &BoxRow, again: bool) -> Result<Then, CommandError> {
    let planned = planned_root(store, row.id)?;
    let input = if again {
        HandsInput::AcquireCut {
            attempted: planned.clone(),
        }
    } else {
        HandsInput::AcquireFailed {
            cause: CAUSE.to_owned(),
            attempted: planned.clone(),
        }
    };
    let lost = Decision::new(row.id).hands_input(row.hands_inc(), input);
    store.apply(lost.effect(if again {
        EffectChange::Again
    } else {
        EffectChange::Keep
    }))?;
    if !again {
        store.apply(gave_up(row))?;
    }
    Ok(match planned {
        Some(handle) => Then::Release { handle, again },
        None if again => Then::Resume,
        None => Then::Rest,
    })
}

/// What ends the cut start of `row` that does not acquire again. A machine that cannot be
/// released may still be there, so its Box fails.
fn gave_up(row: &BoxRow) -> Decision {
    let settled = Decision::new(row.id).effect(EffectChange::End);
    if row.lifecycle != BoxLifecycle::SettingUp {
        return settled;
    }
    settled.lifecycle(BoxLifecycleInput::SetupFailed {
        cause: CAUSE.to_owned(),
        which: None,
    })
}

/// Releases `handle`, what the cut-off acquire of `row` may have created; warns and returns false
/// when it may still be there.
pub(super) fn release_warned(cx: &Cx, store: &Store, row: &BoxRow, handle: &BoxHandle) -> bool {
    match cx.driver().release(handle) {
        Ok(Released::Deleted) => true,
        Ok(Released::AlreadyGone) => {
            let gone =
                Decision::new(row.id).hands_input(row.hands_inc(), HandsInput::DriverMissing);
            store.apply(gone).is_ok()
        }
        Err(e) => {
            cx.warn(Warning {
                code: "box_release_failed".to_owned(),
                message: format!(
                    "could not release {} after its start was interrupted: {e}",
                    row.name.as_str()
                ),
            });
            false
        }
    }
}
