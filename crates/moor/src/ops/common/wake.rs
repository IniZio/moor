//! Wake an Asleep Box and machine before an op acts on them.
//!
//! The caller holds the Box lock and passes a row read after taking it.

use std::collections::HashMap;

use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, Field, Fields};
use moor_core::domain::session_state::SessionInput;
use moor_core::view::sleep::SleepValue;
use moor_core::{BoxReq, ErrorKind};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::autosleep::box_dir;
use crate::ops::common::boxlock::{self, BoxLock};
use crate::ops::common::park::restore_tunnel;
use crate::ops::common::provision::recorded_workdir;
use crate::ops::common::view::{box_view, hide_unsupported_box_sleep};
use crate::ops::start::revive_after_wake;
use crate::store::{BoxRow, Store};

/// Wake the Box and its machine when either is Asleep, leaving the Proxy tunnel parked.
///
/// For callers that need the Box awake but not reachable through the Proxy.
/// A Gone Box is never resumed. A Box keeps its working directory across
/// pause and resume, so a usable stored workdir is reused without asking the
/// Driver. Only a Box with no usable stored workdir is looked up and stored
/// with the wake. When the Driver fails to resume the Box or to find its
/// workdir, nothing is applied and the Box and machine stay Asleep, so the next
/// call tries both again.
/// `Ok(true)` when it woke anything. The Proxy tunnel stays parked and no agent
/// is relaunched; close uses this since it needs neither.
pub(crate) fn wake_box_if_asleep(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
) -> Result<bool, CommandError> {
    wake(cx, store, row).map(|w| w.any)
}

/// Like `wake_box_if_asleep`, then restores the tunnel and relaunches a gone agent.
///
/// A Box can cold-boot on resume and lose its agent process. The tunnel comes
/// back first because the agent reaches the model through the Proxy. When the
/// tunnel or the relaunch fails the Box stays awake and the error is returned.
pub(crate) fn wake_and_revive(cx: &Cx, store: &Store, row: &BoxRow) -> Result<bool, CommandError> {
    let woke = wake_with_tunnel(cx, store, row)?;
    if woke.box_resumed {
        revive_after_wake(cx, store, row.id)?;
    }
    Ok(woke.any)
}

/// Wake the Box and machine, then restore the tunnel when the machine was resumed.
fn wake_with_tunnel(cx: &Cx, store: &Store, row: &BoxRow) -> Result<Woke, CommandError> {
    let woke = wake(cx, store, row)?;
    if woke.box_resumed {
        // The Box stays awake when the tunnel cannot return; the op fails instead.
        restore_tunnel(cx, store, row.id, false)?;
    }
    Ok(woke)
}

/// What `wake` did.
struct Woke {
    /// The Box or the machine was Asleep and is awake now.
    any: bool,
    /// The Driver resumed the Box.
    box_resumed: bool,
}

/// Wake the Box and machine when Asleep and report what was woken.
fn wake(cx: &Cx, store: &Store, row: &BoxRow) -> Result<Woke, CommandError> {
    let row_asleep = row.lifecycle == BoxLifecycle::Asleep;
    let box_asleep = row
        .boxed
        .as_ref()
        .is_some_and(|b| b.state == BoxState::Asleep);
    if !row_asleep && !box_asleep {
        return Ok(Woke {
            any: false,
            box_resumed: false,
        });
    }
    let mut workdir = None;
    if let Some(b) = &row.boxed {
        if matches!(b.state, BoxState::Gone | BoxState::Deleted) {
            return Err(CommandError::new(ErrorKind::Refused, "the Box is gone"));
        }
        if box_asleep {
            let handle = b.handle.as_ref().ok_or_else(|| {
                CommandError::new(ErrorKind::Refused, "the Box has not been acquired")
            })?;
            cx.driver().wake(handle, Some(box_dir(row)))?;
            if b.workdir
                .as_deref()
                .is_none_or(|w| recorded_workdir(w).is_none())
            {
                let worktree = box_dir(row);
                workdir = Some(cx.driver().workdir(handle, worktree)?);
            }
        }
    }
    let mut d = Decision::new(row.id);
    if row_asleep {
        d = d
            .lifecycle(BoxLifecycleInput::Wake)
            .session(SessionInput::AgentBusy);
    }
    if box_asleep {
        d = d.box_input(BoxInput::Wake);
    }
    if let Some(workdir) = workdir {
        d = d.set(Fields {
            box_workdir: Field::Set(workdir.as_str().to_owned()),
            ..Fields::default()
        });
    }
    store.apply(d)?;
    Ok(Woke {
        any: true,
        box_resumed: box_asleep,
    })
}

/// Wake the open Box named by `req` under its Box lock and build the value.
///
/// `held` runs on the woken row while the lock is still held.
pub(crate) fn wake_box(
    cx: &Cx,
    req: &BoxReq,
    op: &str,
    held: impl FnOnce(&BoxRow) -> Result<(), CommandError>,
) -> Result<SleepValue, CommandError> {
    let store = cx.store()?;
    let id = store
        .open_box_by_name(&req.box_name)?
        .ok_or_else(|| {
            CommandError::new(
                ErrorKind::Failed,
                format!("no open Box {}", req.box_name.as_str()),
            )
        })?
        .id;
    let _held = BoxLock::acquire(cx.clock(), cx.state_dir()?, id, op, boxlock::WAIT)?;
    let reload = || {
        store.box_row(id)?.ok_or_else(|| {
            CommandError::new(
                ErrorKind::Failed,
                format!("no Box {}", req.box_name.as_str()),
            )
        })
    };
    let woke = wake_and_revive(cx, store, &reload()?)?;
    let row = reload()?;
    held(&row)?;
    let mut boxed = box_view(&row, cx.config()).driver;
    hide_unsupported_box_sleep(boxed.as_mut(), cx, &mut HashMap::new());
    Ok(SleepValue {
        box_name: req.box_name.as_str().to_owned(),
        driver: boxed,
        woke,
    })
}

#[cfg(test)]
mod tests;
