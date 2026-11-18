//! Auto-sleep: park a Box whose agent has been quiet long enough.
//!
//! Runs only where activity already settles Working and Idle. It never waits for the Box
//! lock and never fails the caller.

use moor_core::activity::AgentState;
use moor_core::decide::sleep::{sleep_due, HoldReason, SleepFacts, SleepVerdict};
use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::decision::{Decision, ExtraEvent, Field, Fields, HandsFields, Wanted};
use moor_core::domain::hands::{HandsInput, HandsState};
use moor_core::domain::session_state::{SessionInput, SessionState};
use moor_core::name::{BoxHandle, BoxId};
use moor_core::seam::clock::Ts;
use moor_core::seam::driver::SleepOutcome;
use moor_core::{ErrorKind, LaunchId, Warning};

use crate::cx::owners::Cmd;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::activity::Reading;
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
    /// It ended its turn but waits on a background job it started.
    Background,
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
        } else if reading
            .activity
            .as_ref()
            .is_some_and(|a| matches!(a.state, AgentState::Waiting { .. }))
        {
            Self::Background
        } else {
            Self::Plain
        }
    }
}

/// Sleep the machine of Box `id` when it has been Idle for `box.sleep_after`, Idle for
/// `box.sleep_after_blocked` while its agent is blocked on a prompt, or awake for `box.max_awake`
/// while Idle, going by `reading`. A Box whose agent works, waits on a background job or is unread
/// is held past `box.max_awake` and sleeps only at `box.max_awake_hard`.
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

/// What auto-sleep does with `row`: sleep it when it has been Idle, or awake, long enough, hold it
/// when it is past `box.max_awake` but busy, or leave it be.
fn due(cx: &Cx, store: &Store, row: &BoxRow, agent: AgentRead) -> Result<Verdict, CommandError> {
    let config = &cx.config().box_;
    if cx.traffic().recent(row.id, cx.clock().now()) {
        return Ok(Verdict::NotYet);
    }
    let awake_since = store.awake_since(row.id)?;
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
        awake_since,
        max_awake: config.max_awake(),
        hard_ceiling: config.max_awake_hard(),
        background_job: agent == AgentRead::Background,
    };
    let awake_for = awake_since.map_or(0, |t| {
        u64::try_from(facts.now.as_millis().saturating_sub(t.as_millis()) / 1000).unwrap_or(0)
    });
    Ok(match sleep_due(&facts) {
        SleepVerdict::Due => Verdict::Due(None),
        SleepVerdict::DueAtCeiling => Verdict::Due(Some(CEILING_REASON)),
        SleepVerdict::Held(why) => Verdict::Held(why, awake_for),
        SleepVerdict::NotYet | SleepVerdict::NotIdle => Verdict::NotYet,
    })
}

/// Why a sleep at `box.max_awake_hard` says it slept.
const CEILING_REASON: &str = "max_awake_hard";

/// What auto-sleep does with a Box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// Leave it alone.
    NotYet,
    /// Sleep it, with this reason when it is not for age.
    Due(Option<&'static str>),
    /// Past `box.max_awake` but kept awake, having been awake for this many seconds.
    Held(HoldReason, u64),
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
    if due(cx, store, &row, agent)? == Verdict::NotYet {
        return Ok(());
    }
    let Some(_turn) = cx.try_turn(id, Cmd::Sleep) else {
        return Ok(());
    };
    let Some(row) = store.box_row(id)? else {
        return Ok(());
    };
    let why = match due(cx, store, &row, agent)? {
        Verdict::NotYet => return Ok(()),
        Verdict::Held(why, awake_for) => return note_held(cx, store, &row, why, awake_for),
        Verdict::Due(why) => why,
    };
    if row.hands.as_ref().is_some_and(|b| b.sleep_reason.is_some()) {
        return Ok(());
    }
    let sleep = || sleep_now(cx, store, &row, (agent, launch), why);
    crate::ops::common::wanted::parking(store, id, Wanted::Sleep, sleep).map(drop)
}

/// Records that `row` is past `box.max_awake` and stays awake for `why`, once for each hold: not
/// again while the newest `sleep_held` of this awake stretch already says `why`.
fn note_held(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    why: HoldReason,
    awake_for_secs: u64,
) -> Result<(), CommandError> {
    let awake_since = store.awake_since(row.id)?;
    let told = store
        .last_event_of(row.id, "sleep_held")?
        .is_some_and(|ev| {
            awake_since.is_none_or(|t| ev.ts >= t)
                && ev.payload.get("reason").and_then(|r| r.as_str()) == Some(why.as_str())
        });
    if !told {
        let held = ExtraEvent::SleepHeld {
            reason: why.as_str().to_owned(),
            awake_for_secs,
        };
        match unless_stale(cx, store.apply(Decision::new(row.id).event(held))) {
            Ok(_) | Err(StoreError::Illegal(_)) => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

/// Sleeps `row`, due to sleep, under its turn and records the outcome.
fn sleep_now(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    (agent, launch): (AgentRead, Option<LaunchId>),
    why: Option<&'static str>,
) -> Result<(), CommandError> {
    let id = row.id;
    let handle = sleep_handle(cx, row)?;
    let settled = agent != AgentRead::Unread && settled(cx, store, row)?;
    if !row.session.is_quiet() {
        // Only the hard ceiling gets here; the sleep ends the agent's pane, so its Session is quiet.
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
    let Some(slept) = sleep_parked(cx, store, row, &handle, settled)? else {
        return Ok(());
    };
    match unless_stale(cx, store.apply(recorded(row, slept, cx.clock().now(), why))) {
        Ok(_) | Err(StoreError::Illegal(_)) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// What records the sleep of `row` that came to `slept` at `now`, for `why` when it is not for
/// age. A sleep at the hard ceiling cuts a working agent, which `row`, read before its Session
/// was made quiet, still shows.
fn recorded(row: &BoxRow, slept: SleepOutcome, now: Ts, why: Option<&'static str>) -> Decision {
    let at_work = (!row.session.is_quiet()).then_some(now.as_millis());
    let inc = row.hands_inc();
    let refusal = |sleep_reason| HandsFields {
        sleep_reason,
        ..HandsFields::default()
    };
    match slept {
        SleepOutcome::Slept | SleepOutcome::Native => Decision::new(row.id)
            .lifecycle(BoxLifecycleInput::Sleep {
                auto: true,
                reason: why.map(str::to_owned),
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
    settled: bool,
) -> Result<Option<SleepOutcome>, CommandError> {
    let id = row.id;
    // Parking takes the Proxy away, and the check runs in the Box.
    if !suspend_check::record(cx, store, (id, row.hands_inc()), settled)? {
        return Ok(None);
    }
    park_tunnel(cx, id)?;
    let slept = cx.driver().sleep(handle);
    if !matches!(slept, Ok(SleepOutcome::Slept | SleepOutcome::Native)) {
        let _ = restore_tunnel(cx, id);
        if let Err(e) = revive_after_wake(cx, store, id) {
            cx.warn(Warning {
                code: "agent_relaunch_failed".to_owned(),
                message: e.error.to_string(),
            });
        }
    }
    Ok(Some(slept?))
}

/// Whether `row`'s agent is read and quiet and no run is in flight, so nothing it started writes to
/// the Box.
pub(crate) fn settled(cx: &Cx, store: &Store, row: &BoxRow) -> Result<bool, CommandError> {
    Ok(row.session.is_quiet()
        && reconcile::run_in_flight(cx, store, row.id)?.is_none()
        && !store.agent_unread(row.id, cx.clock().now())?)
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
