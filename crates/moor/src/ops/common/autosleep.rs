//! Auto-sleep: park a Box whose agent has been quiet long enough.
//!
//! Runs only where activity already settles Working and Idle. It never waits for the Box
//! lock and never fails the caller.

use moor_core::decide::sleep::{sleep_due, SleepFacts, SleepVerdict};
use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::cache_save::{CacheSave, CacheSaveDenied};
use moor_core::domain::decision::{Decision, ExtraEvent, Field, Fields, HandsFields, Wanted};
use moor_core::domain::hands::{HandsInput, HandsState};
use moor_core::domain::session_state::{SessionInput, SessionState};
use moor_core::name::{BoxHandle, BoxId};
use moor_core::seam::clock::Ts;
use moor_core::seam::driver::{PausePublish, SleepOutcome};
use moor_core::{ErrorKind, LaunchId, Warning};

use crate::cx::owners::Cmd;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::activity::Reading;
use crate::ops::common::cache::save_decision;
use crate::ops::common::park::{park_tunnel, restore_tunnel};
use crate::ops::common::reconcile;
use crate::ops::common::stale::unless_stale;
use crate::ops::common::suspend_check;
use crate::ops::start::revive_after_wake;
use crate::store::{BoxRow, Store, StoreError};

/// What the last read of the agent said, as auto-sleep needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentRead {
    /// Read, and no prompt holds it.
    Plain,
    /// It waits on an unanswered prompt or a first-run screen.
    Blocked,
    /// It could not be read, and may have started a turn since it went Idle.
    Unread,
}

impl AgentRead {
    /// How `reading` counts.
    pub(crate) fn of(reading: &Reading) -> Self {
        if reading.blocked() {
            Self::Blocked
        } else if reading.unread() {
            Self::Unread
        } else {
            Self::Plain
        }
    }
}

/// Sleep the machine of Box `id` when it has been Idle for `box.sleep_after`, Idle for
/// `box.sleep_after_blocked` while its agent is blocked on a prompt, or awake for `box.max_awake`,
/// going by `reading`. While its agent is unread, only the awake cap sleeps it.
///
/// An op in flight, a Box that cannot be slept right now and a step the store refuses leave
/// everything as it was. A step taken for the agent lands only while the launch `reading` probed
/// is live. A Box with a recorded refusal is not asked again until it has left Idle. Any other
/// failure becomes a warning.
pub(crate) fn run_after_read(cx: &Cx, store: &Store, id: BoxId, reading: &Reading) {
    let read = (AgentRead::of(reading), reading.launch);
    report(cx, try_sleep(cx, store, id, read));
}

/// [`run_after_read`] on an agent read as `agent`, with no launch to go by.
#[cfg(test)]
pub(crate) fn run(cx: &Cx, store: &Store, id: BoxId, agent: AgentRead) {
    report(cx, try_sleep(cx, store, id, (agent, None)));
}

/// Warns of a failure other than a refusal.
fn report(cx: &Cx, slept: Result<(), CommandError>) {
    match slept {
        Err(e) if e.kind != ErrorKind::Refused => cx.warn(Warning {
            code: "auto_sleep_failed".to_owned(),
            message: e.error.to_string(),
        }),
        _ => {}
    }
}

/// Whether `row` has been Idle, or awake, long enough to sleep.
fn due(cx: &Cx, store: &Store, row: &BoxRow, agent: AgentRead) -> Result<bool, CommandError> {
    let config = &cx.config().box_;
    if cx.traffic().recent(row.id, cx.clock().now()) {
        return Ok(false);
    }
    let facts = SleepFacts {
        lifecycle: row.lifecycle,
        session: if agent == AgentRead::Unread {
            SessionState::Working
        } else {
            row.session
        },
        idle_since: row.idle_since,
        now: cx.clock().now(),
        after: config.sleep_after(),
        blocked: agent == AgentRead::Blocked,
        after_blocked: config.sleep_after_blocked(),
        awake_since: store.awake_since(row.id)?,
        max_awake: config.max_awake(),
    };
    Ok(sleep_due(&facts) == SleepVerdict::Due)
}

/// Sleep the machine once the Box's turn is free and the Box is still due.
fn try_sleep(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    (agent, launch): (AgentRead, Option<LaunchId>),
) -> Result<(), CommandError> {
    let Some(row) = store.box_row(id)? else {
        return Ok(());
    };
    if !due(cx, store, &row, agent)? {
        return Ok(());
    }
    let Some(_turn) = cx.try_turn(id, Cmd::Sleep) else {
        return Ok(());
    };
    let Some(row) = store.box_row(id)? else {
        return Ok(());
    };
    if !due(cx, store, &row, agent)? {
        return Ok(());
    }
    if row.hands.as_ref().is_some_and(|b| b.sleep_reason.is_some()) {
        return Ok(());
    }
    let sleep = || sleep_now(cx, store, &row, (agent, launch));
    crate::ops::common::wanted::parking(store, id, Wanted::Sleep, sleep).map(drop)
}

/// Sleeps `row`, due to sleep, under its turn and records the outcome.
fn sleep_now(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    (agent, launch): (AgentRead, Option<LaunchId>),
) -> Result<(), CommandError> {
    let id = row.id;
    let handle = sleep_handle(cx, row)?;
    let settled = if agent == AgentRead::Unread {
        Err(CacheSaveDenied::AgentUnread)
    } else {
        settled(cx, store, row)?
    };
    if !row.session.is_quiet() {
        // Only the awake cap gets here; the sleep ends the agent's pane, so its Session is quiet.
        let mut quiet = Decision::new(id).session(SessionInput::AgentQuiet);
        if let Some(launch) = launch {
            quiet = quiet.launch(launch);
        }
        match unless_stale(cx, store.apply(quiet)) {
            Ok(Some(_)) => {}
            Ok(None) | Err(StoreError::Illegal(_)) => return Ok(()),
            Err(e) => return Err(e.into()),
        }
    }
    let Some(parked) = sleep_parked(cx, store, row, &handle, settled)? else {
        return Ok(());
    };
    match unless_stale(cx, store.apply(recorded(row, parked, cx.clock().now()))) {
        Ok(_) | Err(StoreError::Illegal(_)) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// What records the sleep of `row` that `parked` came to at `now`. A sleep at the awake limit
/// cuts a working agent, which `row`, read before its Session was made quiet, still shows.
fn recorded(row: &BoxRow, parked: Parked, now: Ts) -> Decision {
    let at_work = (!row.session.is_quiet()).then_some(now.as_millis());
    let inc = row.hands_inc();
    let refusal = |sleep_reason| HandsFields {
        sleep_reason,
        ..HandsFields::default()
    };
    match parked.outcome {
        SleepOutcome::Slept | SleepOutcome::Native => Decision::new(row.id)
            .cache_save(parked.cache_save)
            .lifecycle(BoxLifecycleInput::Sleep {
                auto: true,
                reason: None,
            })
            .hands_input(inc, HandsInput::Sleep)
            .hands(inc, refusal(Field::Clear))
            .set(Fields {
                parked_at_work: at_work.map_or(Field::Keep, Field::Set),
                ..Fields::default()
            }),
        SleepOutcome::CannotSleep(reason) => Decision::new(row.id)
            .hands(inc, refusal(Field::Set(reason.clone())))
            .event(ExtraEvent::StaysUp { reason }),
    }
}

/// What asking the Driver to sleep a Box came to.
pub(crate) struct Parked {
    /// What the Driver did.
    pub(crate) outcome: SleepOutcome,
    /// What the sleep decided about the caches, for its `asleep` event.
    pub(crate) cache_save: Option<CacheSave>,
}

/// Park the Proxy tunnel, end the agent's pane, then ask the Driver to sleep the machine.
///
/// The agent's ssh session would otherwise outlive the Box until its keepalive gave up. A wake
/// finds the pane gone and attaches a new one, or relaunches an agent that did not survive. A Box
/// that stays awake gets its tunnel back and its agent back the same way. Both are best effort: the sleep result is what the caller needs
/// to see.
///
/// `settled` comes from [`settled`], read before anything marked the agent quiet for this sleep.
/// None when `row`'s hands were replaced before the sleep began, so there is nothing to sleep.
pub(crate) fn sleep_parked(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    handle: &BoxHandle,
    settled: Result<(), CacheSaveDenied>,
) -> Result<Option<Parked>, CommandError> {
    let id = row.id;
    let cache_save = save_decision(cx, store, row, settled);
    let publish = if cache_save == Some(CacheSave::Allowed) {
        PausePublish::Allowed
    } else {
        PausePublish::Denied
    };
    if unless_stale(cx, store.set_pause_publish((id, row.hands_inc()), publish))?.is_none() {
        return Ok(None);
    }
    // Parking takes the Proxy away, and the check runs in the Box.
    suspend_check::record(cx, store, (id, row.hands_inc()), settled.is_ok());
    park_tunnel(cx, id)?;
    let slept = cx.driver().sleep(handle, publish);
    if !matches!(slept, Ok(SleepOutcome::Slept | SleepOutcome::Native)) {
        let _ = restore_tunnel(cx, id);
        if let Err(e) = revive_after_wake(cx, store, id) {
            cx.warn(Warning {
                code: "agent_relaunch_failed".to_owned(),
                message: e.error.to_string(),
            });
        }
    }
    Ok(Some(Parked {
        outcome: slept?,
        cache_save,
    }))
}

/// Ok when `row`'s agent is read and quiet and no run is in flight, so nothing it started writes to
/// the Box; which of those fails otherwise.
pub(crate) fn settled(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
) -> Result<Result<(), CacheSaveDenied>, CommandError> {
    Ok(if !row.session.is_quiet() {
        Err(CacheSaveDenied::AgentNotQuiet)
    } else if reconcile::run_in_flight(cx, store, row.id)?.is_some() {
        Err(CacheSaveDenied::RunInFlight)
    } else if store.agent_unread(row.id, cx.clock().now())? {
        Err(CacheSaveDenied::AgentUnread)
    } else {
        Ok(())
    })
}

/// A refusal with `why`.
pub(crate) fn refuse(why: String) -> CommandError {
    CommandError::new(ErrorKind::Refused, why)
}

/// The handle of the ready Box when no run is in flight, or a refusal.
pub(crate) fn sleep_handle(cx: &Cx, row: &BoxRow) -> Result<BoxHandle, CommandError> {
    let handle = ready_handle(row)?;
    if reconcile::run_in_flight(cx, cx.store()?, row.id)?.is_some() {
        return Err(refuse("a run is in flight".to_owned()));
    }
    Ok(handle)
}

/// The handle of the ready Box, or a refusal.
pub(crate) fn ready_handle(row: &BoxRow) -> Result<BoxHandle, CommandError> {
    let b = row
        .hands
        .as_ref()
        .ok_or_else(|| refuse("the Box has no machine".to_owned()))?;
    match b.state {
        HandsState::Ready => {}
        HandsState::Gone | HandsState::Deleted => return Err(refuse("the Box is gone".to_owned())),
        _ => return Err(refuse("the Box is not ready".to_owned())),
    }
    b.handle
        .clone()
        .ok_or_else(|| refuse("the Box has not been acquired".to_owned()))
}

#[cfg(test)]
mod tests;
