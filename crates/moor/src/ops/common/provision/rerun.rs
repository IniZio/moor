//! What a start does in a Box that already exists: warn about a setup that failed or was
//! interrupted, which is never re-run.

use moor_core::decide::intent::setup_dead;
use moor_core::domain::box_state::SetupOutcome;
use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::boxes::BoxState;
use moor_core::domain::decision::{Decision, ExtraEvent, Field, Fields, IntentChange};
use moor_core::domain::session_state::SessionInput;
use moor_core::Warning;

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::view::rfc3339;
use crate::store::{BoxRow, Store};

/// Warns that `moor start` does not re-run a setup that failed or was interrupted in a ready Box.
///
/// An interrupted setup (its owner died) is recorded as a failed one: the intent is cleared and
/// the failure marker set, so `show` reports it and later starts take the failed branch.
pub(super) fn note_setup_not_rerun(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
) -> Result<(), CommandError> {
    if row.boxed.as_ref().map(|b| b.state) != Some(BoxState::Ready) {
        return Ok(());
    }
    let name = row.name.as_str();
    let by_hand = format!("`moor start` does not re-run it; run each `setup:` line by hand with `moor run {name} -- '<line>'`");
    let message = if let Some(intent) = &row.intent {
        if !setup_dead(intent, cx.proc().alive(intent.pid())) {
            return Ok(());
        }
        record_interrupted(cx, store, row)?;
        format!("setup was interrupted; {by_hand}")
    } else if let (BoxLifecycle::Ready, Some(at)) = (row.lifecycle, row.setup_failed_at) {
        format!("setup failed at {}; {by_hand}", rfc3339(at))
    } else {
        return Ok(());
    };
    cx.warn(Warning {
        code: "setup_not_rerun".to_owned(),
        message,
    });
    Ok(())
}

/// Records a dead setup as failed, as a failed setup run would, without running anything.
pub(super) fn record_interrupted(cx: &Cx, store: &Store, row: &BoxRow) -> Result<(), CommandError> {
    let line = "setup was interrupted".to_owned();
    let mut done = Decision::new(row.id);
    if row.lifecycle == BoxLifecycle::SettingUp {
        done = done
            .lifecycle(BoxLifecycleInput::Ready {
                setup: SetupOutcome::Failed { line },
            })
            .session(SessionInput::Started);
    } else {
        done = done.event(ExtraEvent::SetupFailed { line });
    }
    let done = done
        .set(Fields {
            setup_failed_at: Field::Set(cx.clock().now().as_millis()),
            ..Fields::default()
        })
        .intent(IntentChange::Clear);
    store.apply(done)?;
    Ok(())
}
