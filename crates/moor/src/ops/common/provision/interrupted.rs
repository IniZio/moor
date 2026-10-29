//! Fails the starts an earlier controller process left unfinished.

use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, Intent, IntentChange};
use moor_core::seam::driver::Released;
use moor_core::Warning;

use super::lease::planned_root;
use super::rerun::record_interrupted;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::{BoxRow, Store};

/// Why a Box whose acquire was cut off failed.
const CAUSE: &str = "setup interrupted by a controller restart";

/// Fails each open Box whose start no live process still runs, releasing what its acquire planned.
///
/// Called before the controller serves, so no start of this process is under way yet: an intent
/// naming this pid is an earlier process's, as a restarted container reuses its pid.
pub(crate) fn fail_interrupted(cx: &Cx, store: &Store) -> Result<(), CommandError> {
    let me = std::process::id();
    for row in store.open_boxes()? {
        let Some(intent) = row.intent else {
            continue;
        };
        if intent.pid() != me && cx.proc().alive(intent.pid()) {
            continue;
        }
        match intent {
            Intent::Provisioning { .. } => {
                cx.for_box(row.user, || fail_acquire(cx, store, &row))?;
            }
            Intent::Setup { .. } => record_interrupted(cx, store, &row)?,
            Intent::Running { .. } => {}
        }
    }
    Ok(())
}

/// Records the cut-off acquire as failed and releases the machine it may have created.
fn fail_acquire(cx: &Cx, store: &Store, row: &BoxRow) -> Result<(), CommandError> {
    if row.boxed.as_ref().map(|b| b.state) != Some(BoxState::Starting) {
        return Ok(());
    }
    let planned = planned_root(store, row.id)?;
    let mut lost = Decision::new(row.id);
    if row.lifecycle == BoxLifecycle::SettingUp {
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
        .intent(IntentChange::Clear);
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
