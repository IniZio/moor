//! Settles the starts an earlier controller process left unfinished.

use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::decision::{Decision, Intent, IntentChange};
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

/// The open Boxes whose start an earlier controller process left unfinished: an intent other than
/// a run is set, or a launch was cut. The controller lock keeps every other process off the
/// store, so each is dead and resumes as its owner's first turn.
pub(crate) fn cut_short(store: &Store) -> Result<Vec<BoxId>, CommandError> {
    let mut cut = Vec::new();
    for row in store.open_boxes()? {
        let resumes = match row.intent {
            Some(Intent::Running { .. }) => false,
            Some(_) => true,
            None => launch_was_cut(store, &row)?,
        };
        if resumes {
            cut.push(row.id);
        }
    }
    Ok(cut)
}

/// Settles the cut-off start of Box `id` under the Resume turn the caller holds; returns whether
/// [`super::resume_setup`] goes on with it: its setup can be followed again or finished in its
/// machine, its acquire was cut for the first time in this start, its launch was cut, or a forced
/// close wants it gone.
pub(crate) fn settle(cx: &Cx, store: &Store, id: BoxId) -> Result<bool, CommandError> {
    let Some(row) = store.box_row(id)? else {
        return Ok(false);
    };
    let Some(intent) = row.intent else {
        return launch_was_cut(store, &row);
    };
    match intent {
        Intent::Provisioning => cx.for_box(row.user, || cut_acquire(cx, store, &row)),
        Intent::Setup if followable(&row) => Ok(true),
        Intent::Setup => record_interrupted(cx, store, &row, NOT_READY.to_owned()).map(|()| false),
        Intent::Running { .. } => Ok(false),
        Intent::Closing => Ok(true),
    }
}

/// Settles every Box [`cut_short`] finds, each under a Resume turn of its own; returns those that
/// go on, for [`super::resume_setup`].
#[cfg(test)]
pub(crate) fn fail_interrupted(cx: &Cx, store: &Store) -> Result<Vec<BoxId>, CommandError> {
    let mut claimed = Vec::new();
    for id in cut_short(store)? {
        let _turn = cx.turn(id, crate::cx::owners::Cmd::Resume)?;
        if settle(cx, store, id)? {
            claimed.push(id);
        }
    }
    Ok(claimed)
}

/// Whether a start began launching the agent of `row` and no session came of it. A `--no-agent`
/// start never begins, and a launch that failed says so.
pub(super) fn launch_was_cut(store: &Store, row: &BoxRow) -> Result<bool, CommandError> {
    let ready = row
        .hands
        .as_ref()
        .is_some_and(|b| b.state == HandsState::Ready);
    let waits = row.lifecycle == BoxLifecycle::Ready && !row.session.is_quiet();
    if !ready || !waits || row.launch_failure.is_some() || store.live_session(row.id)?.is_some() {
        return Ok(false);
    }
    let events = store.events(row.id, usize::MAX)?;
    let began = events.iter().rfind(|e| {
        e.kind == "step_started" && e.payload.get("step").and_then(|s| s.as_str()) == Some("launch")
    });
    let began = began.map_or(0, |e| e.id);
    let session = events.iter().rfind(|e| e.kind == "session_started");
    Ok(began > session.map_or(0, |e| e.id))
}

/// Whether the cut-off start of `row` was setting up in a machine it still holds: running its
/// setup lines, or the steps after them once every line passed.
fn followable(row: &BoxRow) -> bool {
    row.lifecycle == BoxLifecycle::SettingUp
        && row.hands.as_ref().is_some_and(|b| {
            b.state == HandsState::Ready && b.handle.is_some() && b.workdir.is_some()
        })
}

/// Settles a cut-off acquire; returns whether this process goes on with it.
fn cut_acquire(cx: &Cx, store: &Store, row: &BoxRow) -> Result<bool, CommandError> {
    let setting_up = row.lifecycle == BoxLifecycle::SettingUp;
    let claim = IntentChange::Set(Intent::Provisioning);
    match row.hands.as_ref().map(|b| b.state) {
        Some(HandsState::Starting) => {
            let planned = planned_root(store, row.id)?;
            let goes_on = planned
                .as_ref()
                .is_some_and(|h| cx.driver().acquire_outlives_caller(h));
            if goes_on && setting_up {
                store.apply(Decision::new(row.id).intent(claim))?;
                return Ok(true);
            }
            if goes_on {
                // Stopping it can wait for it, and the controller is not serving yet.
                let lost = Decision::new(row.id)
                    .hands_input(HandsInput::AcquireFailed {
                        cause: CAUSE.to_owned(),
                        attempted: planned,
                    })
                    .intent(claim);
                store.apply(lost)?;
                return Ok(true);
            }
            fail_acquire(cx, store, row, setting_up && !cut_before(store, row)?)
        }
        // The process that claimed it to acquire again ended before it could.
        Some(HandsState::Failed | HandsState::Gone) if setting_up => {
            let lost = Decision::new(row.id)
                .lifecycle(BoxLifecycleInput::SetupFailed {
                    cause: CAUSE.to_owned(),
                    which: None,
                })
                .intent(IntentChange::Clear);
            store.apply(lost)?;
            Ok(false)
        }
        _ => Ok(false),
    }
}

/// Whether an acquire of this start already failed, as only a cut one can and still be claimed.
fn cut_before(store: &Store, row: &BoxRow) -> Result<bool, CommandError> {
    let newest = |kind| -> Result<i64, CommandError> {
        let found = store.newest_events_of(row.id, kind, 1)?;
        Ok(found.first().map_or(0, |e| e.id))
    };
    Ok(newest("box_failed")? > newest("setting_up")?)
}

/// Records the cut-off acquire as failed and releases the machine it may have created; returns
/// whether this process acquires again, which `again` asks for. A machine that cannot be released
/// may still be there, so its Box fails instead.
fn fail_acquire(cx: &Cx, store: &Store, row: &BoxRow, again: bool) -> Result<bool, CommandError> {
    let planned = planned_root(store, row.id)?;
    let lost = Decision::new(row.id).hands_input(HandsInput::AcquireFailed {
        cause: CAUSE.to_owned(),
        attempted: planned.clone(),
    });
    store.apply(lost.intent(IntentChange::Set(Intent::Provisioning)))?;
    let released = planned
        .as_ref()
        .is_none_or(|handle| release_warned(cx, store, row, handle));
    if again && released {
        return Ok(true);
    }
    let mut settled = Decision::new(row.id).intent(IntentChange::Clear);
    if row.lifecycle == BoxLifecycle::SettingUp {
        settled = settled.lifecycle(BoxLifecycleInput::SetupFailed {
            cause: CAUSE.to_owned(),
            which: None,
        });
    }
    store.apply(settled)?;
    Ok(false)
}

/// Releases `handle`, what the cut-off acquire of `row` may have created; warns and returns false
/// when it may still be there.
pub(super) fn release_warned(cx: &Cx, store: &Store, row: &BoxRow, handle: &BoxHandle) -> bool {
    match cx.driver().release(handle) {
        Ok(Released::Deleted) => true,
        Ok(Released::AlreadyGone) => {
            let gone = Decision::new(row.id).hands_input(HandsInput::DriverMissing);
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
