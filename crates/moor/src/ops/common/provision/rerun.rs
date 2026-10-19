//! What a start does in a Box that already exists: sync the worktree into it, and warn about a
//! setup that failed or was interrupted, which is never re-run.

use moor_core::config::Flags;
use moor_core::decide::intent::setup_dead;
use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::boxes::BoxState;
use moor_core::domain::decision::{Decision, ExtraEvent, Field, Fields, IntentChange};
use moor_core::domain::run::SyncMode;
use moor_core::domain::session_state::SessionInput;
use moor_core::domain::task::{SetupOutcome, TaskState};
use moor_core::Warning;
use std::path::Path;

use super::outcome::sync_failed;
use super::{moor_yaml, recorded_workdir};
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::boxwork::{self, BoxWork};
use crate::ops::common::facts::resolved;
use crate::ops::common::setup;
use crate::ops::common::view::rfc3339;
use crate::store::{Store, TaskRow};

/// Decides whether a start syncs the host checkout into the Task's existing Box.
///
/// A sync resets the Box checkout to the host's: HEAD, branches and remote-tracking refs move
/// even when every commit is pushed. A Task that has had an agent session is therefore synced
/// only when the Box holds no work that exists nowhere else, or has no git checkout at all
/// (an earlier sync failed); otherwise the sync is skipped with a warning. While an agent session
/// is live no sync is made at all: the agent may be writing files in the checkout.
pub(super) fn rerun_sync(
    cx: &Cx,
    store: &Store,
    row: &TaskRow,
    gates: &ResyncGates,
) -> Result<SyncMode, CommandError> {
    if (gates.agent_live)()? {
        cx.warn(Warning {
            code: "resync_skipped".to_owned(),
            message: "did not sync the checkout into the Box: an agent session is still running"
                .to_owned(),
        });
        return Ok(SyncMode::None);
    }
    if !store.has_session(row.id)? {
        return Ok(SyncMode::Full);
    }
    let Some(boxed) = row.boxed.as_ref() else {
        return Ok(SyncMode::None);
    };
    let why = match boxwork::probe(cx, row, boxed) {
        BoxWork::Clean | BoxWork::NoCheckout => return Ok(SyncMode::Full),
        BoxWork::Unsaved { ahead, dirty } => {
            format!("the Box has {ahead} commits the host lacks and {dirty} changed paths")
        }
        BoxWork::Gone => "the Box is gone".to_owned(),
        BoxWork::Unknown(reason) => {
            format!("cannot tell whether the Box has unsaved work: {reason}")
        }
    };
    cx.warn(Warning {
        code: "resync_skipped".to_owned(),
        message: format!("did not sync the checkout into the Box: {why}"),
    });
    Ok(SyncMode::None)
}

/// What a caller knows that decides a re-sync of the checkout.
pub(crate) struct ResyncGates<'a> {
    /// Whether the caller is about to launch the agent; asked only if the re-sync fails.
    pub launching: &'a dyn Fn() -> Result<bool, CommandError>,
    /// Whether an agent session is still running.
    pub agent_live: &'a dyn Fn() -> Result<bool, CommandError>,
}

/// Syncs the host checkout into a ready Box.
///
/// A failed sync ends the start like a fresh
/// one's when the agent is about to launch; otherwise it is a warning and the start goes on, so
/// the user can retry.
///
/// # Errors
/// The sync failed while launching, or the repo config could not be read.
pub(super) fn resync(
    cx: &Cx,
    store: &Store,
    row: &TaskRow,
    trusted: bool,
    gates: &ResyncGates,
) -> Result<(), CommandError> {
    let Some(boxed) = row.boxed.as_ref().filter(|b| b.state == BoxState::Ready) else {
        return Ok(());
    };
    let (Some(handle), Some(workdir)) = (
        boxed.handle.as_ref(),
        boxed.workdir.as_deref().and_then(recorded_workdir),
    ) else {
        return Ok(());
    };
    if rerun_sync(cx, store, row, gates)? != SyncMode::Full {
        return Ok(());
    }
    let repo = moor_yaml::for_teardown(cx, row, trusted)?;
    let cfg = resolved(cx, repo.as_ref(), &Flags::default());
    let worktree = Path::new(row.worktree.as_deref().unwrap_or(&row.repo));
    let Err(reason) = setup::sync_checkout(cx, handle, &workdir, worktree, &cfg) else {
        return Ok(());
    };
    if (gates.launching)()? {
        let advance = row.state == TaskState::SettingUp;
        return Err(sync_failed(cx, store, row.id, advance, handle, reason));
    }
    cx.warn(Warning {
        code: "resync_failed".to_owned(),
        message: format!("{reason}; run `moor start` again"),
    });
    Ok(())
}

/// Warns that `moor start` does not re-run a setup that failed or was interrupted in a ready Box.
///
/// An interrupted setup (its owner died) is recorded as a failed one: the intent is cleared and
/// the failure marker set, so `show` reports it and later starts take the failed branch.
pub(super) fn note_setup_not_rerun(
    cx: &Cx,
    store: &Store,
    row: &TaskRow,
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
    } else if let (TaskState::Working | TaskState::Idle, Some(at)) =
        (row.state, row.setup_failed_at)
    {
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
fn record_interrupted(cx: &Cx, store: &Store, row: &TaskRow) -> Result<(), CommandError> {
    let line = "setup was interrupted".to_owned();
    let mut done = Decision::new(row.id);
    if row.state == TaskState::SettingUp {
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
