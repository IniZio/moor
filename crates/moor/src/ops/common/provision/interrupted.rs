//! Fails the starts an earlier controller process left unfinished.

use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, Intent, IntentChange};
use moor_core::seam::driver::Released;
use moor_core::{BoxId, Warning};

use super::lease::planned_root;
use super::rerun::record_interrupted;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::{BoxRow, Store};

/// Why a Box whose acquire was cut off failed.
const CAUSE: &str = "setup interrupted by a controller restart";

/// Settles each open Box whose start no live process still runs, and claims for this process each
/// Box that can go on: its setup lines can be followed again in its machine, or its acquire was cut
/// for the first time in this start. Returns the claimed Boxes, for [`super::resume_setup`].
///
/// Called before the controller serves, so no start of this process is under way yet: an intent
/// naming this pid is an earlier process's, as a restarted container reuses its pid.
pub(crate) fn fail_interrupted(cx: &Cx, store: &Store) -> Result<Vec<BoxId>, CommandError> {
    let me = std::process::id();
    let mut claimed = Vec::new();
    for row in store.open_boxes()? {
        let Some(intent) = row.intent else {
            continue;
        };
        if intent.pid() != me && cx.proc().alive(intent.pid()) {
            continue;
        }
        match intent {
            Intent::Provisioning { .. } => {
                if cx.for_box(row.user, || cut_acquire(cx, store, &row, me))? {
                    claimed.push(row.id);
                }
            }
            Intent::Setup { .. } if followable(&row) => {
                let mine = IntentChange::Set(Intent::Setup { pid: me });
                store.apply(Decision::new(row.id).intent(mine))?;
                claimed.push(row.id);
            }
            Intent::Setup { .. } => record_interrupted(cx, store, &row)?,
            Intent::Running { .. } => {}
        }
    }
    Ok(claimed)
}

/// Whether the cut-off start of `row` was running its setup lines in a machine it still holds.
fn followable(row: &BoxRow) -> bool {
    row.lifecycle == BoxLifecycle::SettingUp
        && row.steps_pending_at.is_none()
        && row.boxed.as_ref().is_some_and(|b| {
            b.state == BoxState::Ready && b.handle.is_some() && b.workdir.is_some()
        })
}

/// Settles a cut-off acquire; returns whether this process acquires once more.
fn cut_acquire(cx: &Cx, store: &Store, row: &BoxRow, me: u32) -> Result<bool, CommandError> {
    let setting_up = row.lifecycle == BoxLifecycle::SettingUp;
    match row.boxed.as_ref().map(|b| b.state) {
        Some(BoxState::Starting) => {
            let again = (setting_up && !cut_before(store, row)?).then_some(me);
            fail_acquire(cx, store, row, again)?;
            Ok(again.is_some())
        }
        // The process that claimed it to acquire again ended before it could.
        Some(BoxState::Failed | BoxState::Gone) if setting_up => {
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

/// Records the cut-off acquire as failed and releases the machine it may have created. With
/// `again` that process keeps the Box's start, so its setup has not failed.
fn fail_acquire(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    again: Option<u32>,
) -> Result<(), CommandError> {
    let planned = planned_root(store, row.id)?;
    let mut lost = Decision::new(row.id);
    if row.lifecycle == BoxLifecycle::SettingUp && again.is_none() {
        lost = lost.lifecycle(BoxLifecycleInput::SetupFailed {
            cause: CAUSE.to_owned(),
            which: None,
        });
    }
    let lost = lost
        .box_input(BoxInput::AcquireFailed {
            cause: CAUSE.to_owned(),
            attempted: planned.clone(),
        })
        .intent(again.map_or(IntentChange::Clear, |pid| {
            IntentChange::Set(Intent::Provisioning { pid })
        }));
    store.apply(lost)?;
    let Some(handle) = planned else {
        return Ok(());
    };
    match cx.driver().release(&handle) {
        Ok(Released::AlreadyGone) => {
            store.apply(Decision::new(row.id).box_input(BoxInput::DriverMissing))?;
        }
        Ok(Released::Deleted) => {}
        Err(e) => cx.warn(Warning {
            code: "box_release_failed".to_owned(),
            message: format!(
                "could not release {} after its start was interrupted: {e}",
                row.name.as_str()
            ),
        }),
    }
    Ok(())
}
