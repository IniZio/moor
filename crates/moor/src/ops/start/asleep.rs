//! Rouses an asleep or Idle Box that a start attaches to, before its agent launches.

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::boxes::BoxState;
use moor_core::domain::decision::Decision;
use moor_core::domain::session_state::SessionInput;
use moor_core::{BoxId, StartReq};

use super::relaunch;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::boxlock::{self, BoxLock};
use crate::ops::common::park::restore_tunnel;
use crate::ops::common::provision::reread;
use crate::ops::common::wake::wake_box_if_asleep;
use crate::store::{Store, StoreError};

/// Whether an attached Box can go on to launch its agent.
#[derive(PartialEq, Eq)]
pub(super) enum Woken {
    /// The Box is awake, or was never asleep.
    Awake,
    /// Its node has no room to wake it, so it waits Pending with the reason recorded.
    Queued,
}

/// Wakes an asleep Box being started and brings its tunnel back, since the agent launch that
/// follows needs both. A node without room leaves it queued, holding any given prompt for later.
pub(super) fn wake(
    cx: &Cx,
    store: &Store,
    req: &StartReq,
    id: BoxId,
) -> Result<Woken, CommandError> {
    let _held = BoxLock::acquire(cx.clock(), cx.state_dir()?, id, "start", boxlock::WAIT)?;
    let row = reread(store, id)?;
    let machine_asleep = row
        .boxed
        .as_ref()
        .is_some_and(|b| b.state == BoxState::Asleep);
    if let Err(e) = wake_box_if_asleep(cx, store, &row) {
        if reread(store, id)?.lifecycle != BoxLifecycle::Pending {
            return Err(e);
        }
        relaunch::store_prompt(store, req, id)?;
        return Ok(Woken::Queued);
    }
    if machine_asleep {
        restore_tunnel(cx, id)?;
    }
    Ok(Woken::Awake)
}

/// Moves an Idle Box being started to Working, so the launch check sees a Working Box.
pub(super) fn wake_idle(store: &Store, id: BoxId) -> Result<(), CommandError> {
    let row = reread(store, id)?;
    if !(row.lifecycle == BoxLifecycle::Ready && row.session.is_quiet()) {
        return Ok(());
    }
    match store.apply(Decision::new(id).session(SessionInput::AgentBusy)) {
        Ok(_) | Err(StoreError::Illegal(_)) => Ok(()),
        Err(e) => Err(e.into()),
    }
}
