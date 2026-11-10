//! What a start does in a Box that already exists: finish the steps a restart cut off after the
//! setup lines passed, or warn about a setup that failed or was interrupted, which is never re-run.

use moor_core::decide::effect::setup_dead;
use moor_core::domain::box_state::SetupOutcome;
use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::decision::{Decision, EffectChange, ExtraEvent, Field, Fields};
use moor_core::domain::hands::HandsState;

use moor_core::{ErrorKind, Warning};

use moor_core::config::Flags;

use super::after_lines::after_setup;
use super::outcome::{dotfiles_noted, setup_ended};
use super::Provisioned;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::facts::settings_of;
use crate::ops::common::setup::SetupRun;
use crate::ops::common::view::rfc3339;
use crate::ops::common::{agent_install, box_exec};
use crate::store::{BoxRow, Store};

/// Runs the steps a restart cut off after the setup lines passed in a ready Box, or warns that a
/// setup that failed or was interrupted mid-line is not re-run.
pub(super) fn start_ready(cx: &Cx, store: &Store, row: &BoxRow) -> Result<(), CommandError> {
    if row.hands.as_ref().map(|b| b.state) != Some(HandsState::Ready) {
        return Ok(());
    }
    if let Some(effect) = &row.effect {
        if !setup_dead(effect, cx.owners().busy(row.id)) {
            return Ok(());
        }
        record_interrupted(cx, store, row, OWNER_DIED.to_owned())?;
    }
    if row.steps_pending_at.is_some() && row.lifecycle != BoxLifecycle::SettingUp {
        return finish_steps(cx, store, row, false).map(drop);
    }
    let name = row.name.as_str();
    let by_hand = format!("`moor start` does not re-run it; run each `setup:` line by hand with `moor run {name} -- '<line>'`");
    let message = if row.effect.is_some() && row.lifecycle == BoxLifecycle::SettingUp {
        format!("setup was interrupted; retry it with `moor retry {name}`")
    } else if row.effect.is_some() {
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
/// `advance` moves a Box still setting up on to working, or to setup-failed, and ends its effect.
pub(super) fn finish_steps(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    advance: bool,
) -> Result<Provisioned, CommandError> {
    let Some((held, handle)) = row
        .hands
        .as_ref()
        .and_then(|b| Some((b, b.handle.as_ref()?)))
    else {
        return Err(CommandError::new(
            ErrorKind::Failed,
            "the Box holds no machine to finish its setup steps in",
        ));
    };
    let cfg = settings_of(cx, row, &Flags::default())?;
    let repo = row.repo.as_ref().map(|r| &r.repo);
    let wants = repo.is_some() || !cfg.setup.is_empty() || cfg.dotfiles.is_some();
    let proxy = wants
        .then(|| box_exec::proxy(cx, row.id, Some((handle.clone(), held.inc)), repo, &cfg))
        .transpose();
    let at = (handle, held.provider.as_str());
    // The cut process may have finished the install, and running it again races what it left
    // going in the Box.
    let install = !agent_install::present(cx, at, row.id, &cfg, &proxy);
    let after = after_setup(cx, store, at, row.id, &cfg, &proxy, install)?;
    let mut set = Fields {
        steps_pending_at: Field::Clear,
        ..Fields::default()
    };
    let install_failed = after.installed.is_err();
    let why = after.installed.as_ref().err().cloned();
    let outcome = match after.installed {
        Ok(()) => SetupOutcome::Ok,
        Err(line) => {
            set.setup_failed_at = Field::Set(cx.clock().now().as_millis());
            SetupOutcome::Failed { line }
        }
    };
    let mut done = setup_ended(dotfiles_noted(row.id, &after.dotfiles), advance, outcome)
        .setup_why(why)
        .set(set);
    if advance {
        done = done.effect(EffectChange::End);
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

/// Why a setup a restart cut is recorded as failed when its machine cannot go on with it.
pub(super) const NOT_READY: &str =
    "the machine was not ready to go on with the setup after a controller restart";

/// Records a dead setup as failed for `why`, as a failed setup run would, without running
/// anything, so the Box waits for a retry; a Box already working only ends the effect, and its
/// later steps stay pending.
pub(super) fn record_interrupted(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    why: String,
) -> Result<(), CommandError> {
    if row.steps_pending_at.is_some() && row.lifecycle != BoxLifecycle::SettingUp {
        store.apply(Decision::new(row.id).effect(EffectChange::End))?;
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
            steps_pending_at: Field::Clear,
            ..Fields::default()
        })
        .effect(EffectChange::End);
    store.apply(done)?;
    Ok(())
}
