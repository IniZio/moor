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

use super::after_lines::after_setup;
use super::outcome::{dotfiles_noted, setup_ended};
use super::Provisioned;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::box_exec;
use crate::ops::common::facts::settings_of;
use crate::ops::common::setup::SetupRun;
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
        record_interrupted(cx, store, row, OWNER_DIED.to_owned())?;
    }
    if row.steps_pending_at.is_some() {
        return finish_steps(cx, store, row, false).map(drop);
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
/// `advance` moves a Box still setting up on to working, or to setup-failed, and ends its intent.
pub(super) fn finish_steps(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    advance: bool,
) -> Result<Provisioned, CommandError> {
    let Some((held, handle)) = row
        .boxed
        .as_ref()
        .and_then(|b| Some((b, b.handle.as_ref()?)))
    else {
        return Ok(Provisioned::skipped());
    };
    let cfg = settings_of(cx, row, &Flags::default())?;
    let repo = row.repo.as_ref().map(|r| &r.repo);
    let wants = repo.is_some() || !cfg.setup.is_empty() || cfg.dotfiles.is_some();
    let proxy = wants
        .then(|| box_exec::proxy(cx, row.id, Some(handle.clone()), repo, &cfg))
        .transpose();
    let at = (handle, held.provider.as_str());
    let after = after_setup(cx, store, at, row.id, &cfg, &proxy)?;
    let mut set = Fields {
        steps_pending_at: Field::Clear,
        ..Fields::default()
    };
    let install_failed = after.installed.is_err();
    let outcome = match after.installed {
        Ok(()) => SetupOutcome::Ok,
        Err(line) => {
            set.setup_failed_at = Field::Set(cx.clock().now().as_millis());
            SetupOutcome::Failed { line }
        }
    };
    let mut done = setup_ended(dotfiles_noted(row.id, &after.dotfiles), advance, outcome).set(set);
    if advance {
        done = done.intent(IntentChange::Clear);
    }
    store.apply(done)?;
    Ok(Provisioned {
        setup: SetupRun::Skipped,
        dotfiles: after.dotfiles,
        work_tree: None,
        install_failed,
        warm_disk: None,
    })
}

/// Why a setup whose controller process died is recorded as failed.
pub(super) const OWNER_DIED: &str =
    "a controller restart or crash stopped this setup before it finished";

/// Records a dead setup as failed for `why`, as a failed setup run would, without running
/// anything, so the Box waits for a retry; once every line had passed, the Box is ready and only
/// the later steps stay pending.
pub(super) fn record_interrupted(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    why: String,
) -> Result<(), CommandError> {
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
        done = done
            .lifecycle(BoxLifecycleInput::SetupFailed {
                cause: line,
                which: None,
            })
            .setup_why(Some(why));
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
