//! What a start does in a Box that already exists: finish the steps a restart cut off after the
//! setup lines passed, or warn about a setup that failed or was interrupted, which is never re-run.

use moor_core::decide::intent::setup_dead;
use moor_core::domain::box_state::SetupOutcome;
use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::boxes::BoxState;
use moor_core::domain::decision::{Decision, ExtraEvent, Field, Fields, IntentChange};
use moor_core::domain::session_state::SessionInput;
use moor_core::Warning;

use moor_core::config::Flags;

use super::after_setup;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::box_exec;
use crate::ops::common::dotfiles::DotfilesRun;
use crate::ops::common::facts::settings_of;
use crate::ops::common::view::rfc3339;
use crate::store::{BoxRow, Store};

/// Runs the steps a restart cut off after the setup lines passed in a ready Box, or warns that a
/// setup that failed or was interrupted mid-line is not re-run.
pub(super) fn start_ready(cx: &Cx, store: &Store, row: &BoxRow) -> Result<(), CommandError> {
    if row.boxed.as_ref().map(|b| b.state) != Some(BoxState::Ready) {
        return Ok(());
    }
    if let Some(intent) = &row.intent {
        if !setup_dead(intent, cx.proc().alive(intent.pid())) {
            return Ok(());
        }
        record_interrupted(cx, store, row)?;
    }
    if row.steps_pending_at.is_some() {
        return finish_steps(cx, store, row);
    }
    let name = row.name.as_str();
    let by_hand = format!("`moor start` does not re-run it; run each `setup:` line by hand with `moor run {name} -- '<line>'`");
    let message = if row.intent.is_some() && row.lifecycle == BoxLifecycle::SettingUp {
        format!("setup was interrupted; retry it with `moor retry {name}`")
    } else if row.intent.is_some() {
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

/// Runs the steps after the setup lines, which all passed, in the Box a restart cut them off in.
fn finish_steps(cx: &Cx, store: &Store, row: &BoxRow) -> Result<(), CommandError> {
    let Some((held, handle)) = row
        .boxed
        .as_ref()
        .and_then(|b| Some((b, b.handle.as_ref()?)))
    else {
        return Ok(());
    };
    let cfg = settings_of(cx, row, &Flags::default())?;
    let repo = row.repo.as_ref().map(|r| &r.repo);
    let wants = repo.is_some() || !cfg.setup.is_empty() || cfg.dotfiles.is_some();
    let proxy = wants
        .then(|| box_exec::proxy(cx, row.id, Some(handle.clone()), repo, &cfg))
        .transpose();
    let at = (handle, held.provider.as_str());
    let dotfiles = after_setup(cx, store, at, row.id, &cfg, &proxy)?;
    let mut done = Decision::new(row.id).set(Fields {
        steps_pending_at: Field::Clear,
        ..Fields::default()
    });
    if let DotfilesRun::Failed { tail } = &dotfiles {
        done = done.event(ExtraEvent::SetupFailed {
            line: format!("dotfiles: {tail}"),
        });
    }
    store.apply(done)?;
    Ok(())
}

/// Records a dead setup as failed, as a failed setup run would, without running anything, so the
/// Box waits for a retry; once every line had passed, the Box is ready and only the later steps
/// stay pending.
pub(super) fn record_interrupted(cx: &Cx, store: &Store, row: &BoxRow) -> Result<(), CommandError> {
    if row.steps_pending_at.is_some() {
        let mut done = Decision::new(row.id).intent(IntentChange::Clear);
        if row.lifecycle == BoxLifecycle::SettingUp {
            done = done
                .lifecycle(BoxLifecycleInput::Ready {
                    setup: SetupOutcome::Ok,
                })
                .session(SessionInput::Started);
        }
        store.apply(done)?;
        return Ok(());
    }
    let line = "setup was interrupted".to_owned();
    let mut done = Decision::new(row.id);
    if row.lifecycle == BoxLifecycle::SettingUp {
        done = done.lifecycle(BoxLifecycleInput::SetupFailed {
            cause: line,
            which: None,
        });
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
