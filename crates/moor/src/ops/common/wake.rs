//! Wake an Asleep Box and machine before an op acts on them.
//!
//! The caller holds the Box lock and passes a row read after taking it.

use std::collections::HashMap;

use moor_core::decide::reconcile::STOPPED_REASON;
use moor_core::domain::box_state::{BoxLifecycle, BoxLifecycleInput};
use moor_core::domain::boxes::{BoxInput, BoxState};
use moor_core::domain::decision::{Decision, Field, Fields};
use moor_core::domain::session_state::SessionInput;
use moor_core::view::sleep::SleepValue;
use moor_core::{BoxReq, ErrorKind, Warning};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::boxlock::{self, BoxLock};
use crate::ops::common::node::now;
use crate::ops::common::park::restore_tunnel;
use crate::ops::common::provision::recorded_workdir;
use crate::ops::common::resume_gate::{room_to_wake, Turn};
use crate::ops::common::view::{box_view, hide_unsupported_box_sleep};
use crate::ops::start::{revive_after_wake, AfterWake};
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
/// is relaunched.
pub(crate) fn wake_box_if_asleep(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
) -> Result<bool, CommandError> {
    wake(cx, store, row, Turn::JoinsQueue).map(|w| w.any)
}

/// Like `wake_box_if_asleep` for a close, which needs neither tunnel nor agent, and which refuses
/// when the Box does not fit now, leaving it as it was.
pub(crate) fn wake_to_close(cx: &Cx, store: &Store, row: &BoxRow) -> Result<bool, CommandError> {
    wake(cx, store, row, Turn::OnlyIfRoom).map(|w| w.any)
}

/// Like `wake_box_if_asleep`, then restores the tunnel and relaunches a gone agent.
///
/// A Box can cold-boot on resume and lose its agent process. The tunnel comes
/// back first because the agent reaches the model through the Proxy. When the
/// tunnel or the relaunch fails the Box stays awake and the error is returned.
pub(crate) fn wake_and_revive(cx: &Cx, store: &Store, row: &BoxRow) -> Result<bool, CommandError> {
    revive(cx, store, row, Turn::JoinsQueue)
}

/// `wake_and_revive` for a promotion, which goes down the queue itself and so reaches a Box only
/// when those ahead have left it or failed on their own account.
pub(crate) fn wake_in_turn(cx: &Cx, store: &Store, row: &BoxRow) -> Result<bool, CommandError> {
    revive(cx, store, row, Turn::Reached)
}

/// Wakes the Box and restores its tunnel and agent.
fn revive(cx: &Cx, store: &Store, row: &BoxRow, turn: Turn) -> Result<bool, CommandError> {
    let woke = wake_with_tunnel(cx, store, row, turn)?;
    if woke.box_resumed {
        let up_at = now(cx);
        let after = revive_after_wake(cx, store, row.id)?;
        tell_after_suspend(cx, store, row, up_at, after)?;
    }
    Ok(woke.any)
}

/// Tells the agent of a Box that stopped mid-turn, once, what that cost it and to carry on. The
/// mark stays until the Box answers, since only then is there something true to say.
fn tell_after_suspend(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    up_at: i64,
    after: AfterWake,
) -> Result<(), CommandError> {
    let Some(at) = store.resume_first(row.id)? else {
        return Ok(());
    };
    let cost = match after {
        AfterWake::Intact => {
            "Its processes and files are intact, but network connections open at that moment \
             were dropped (API calls, downloads, git pushes)."
        }
        AfterWake::Relaunched => {
            "Its processes were lost and its agent was started again, so anything that was \
             running has stopped; its files are kept."
        }
        AfterWake::Unknown => return Ok(()),
    };
    let secs = up_at.saturating_sub(at);
    let stopped = store.newest_event_of(row.id, "asleep")?.is_some_and(|ev| {
        ev.payload.get("reason").and_then(|r| r.as_str()) == Some(STOPPED_REASON)
    });
    let why = if stopped {
        format!("This Box was down for {secs}s: {STOPPED_REASON}.")
    } else {
        format!("This Box was suspended for {secs}s because host memory ran low.")
    };
    let text =
        format!("{why} {cost} Carry on with what you were doing. Re-run anything that failed.");
    if let Err(e) = crate::ops::steer::tell(cx, store, row, text) {
        cx.warn(Warning {
            code: "note_failed".to_owned(),
            message: format!(
                "cannot tell {} about its suspension: {}",
                row.name.as_str(),
                e.error
            ),
        });
    }
    store.set_resume_first(row.id, None)?;
    Ok(())
}

/// Wake the Box and machine, then restore the tunnel when the machine was resumed.
fn wake_with_tunnel(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    turn: Turn,
) -> Result<Woke, CommandError> {
    let woke = wake(cx, store, row, turn)?;
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
fn wake(cx: &Cx, store: &Store, row: &BoxRow, turn: Turn) -> Result<Woke, CommandError> {
    let box_asleep = row
        .boxed
        .as_ref()
        .is_some_and(|b| b.state == BoxState::Asleep);
    let row_asleep = row.lifecycle == BoxLifecycle::Asleep
        || (row.lifecycle == BoxLifecycle::Pending && box_asleep);
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
            if let Some(reason) = room_to_wake(cx, store, row, turn)? {
                return Err(match turn {
                    Turn::OnlyIfRoom => CommandError::new(ErrorKind::Refused, reason),
                    Turn::Reached | Turn::JoinsQueue => wait_for_room(store, row, reason)?,
                });
            }
            cx.driver().wake(handle)?;
            if b.workdir
                .as_deref()
                .is_none_or(|w| recorded_workdir(w).is_none())
            {
                workdir = Some(cx.driver().workdir(handle)?);
            }
        }
    }
    let mut d = Decision::new(row.id);
    if row_asleep {
        d = d.lifecycle(BoxLifecycleInput::Wake);
        // A Box suspended mid-turn wakes with its Session still Working.
        if row.session.is_quiet() {
            d = d.session(SessionInput::AgentBusy);
        }
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

/// Leaves an asleep Box Pending for `reason` until its node has room; a promotion wakes it then.
pub(crate) fn wait_for_room(
    store: &Store,
    row: &BoxRow,
    reason: String,
) -> Result<CommandError, CommandError> {
    let mut d = Decision::new(row.id).set(Fields {
        state_reason: Field::Set(reason.clone()),
        ..Fields::default()
    });
    if row.lifecycle == BoxLifecycle::Asleep {
        d = d.lifecycle(BoxLifecycleInput::WaitForRoom);
    }
    store.apply(d)?;
    Ok(CommandError::new(ErrorKind::Refused, reason)
        .with_hint("it wakes on its own once its node has room"))
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
