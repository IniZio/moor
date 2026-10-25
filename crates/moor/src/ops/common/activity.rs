//! The one read of a Box's agent: probe once, decide, keep the hook cache.

use std::time::Duration;

use moor_core::activity::{
    decide, may_apply_quiet, Activity, AgentState, HookRead, PaneRead, PaneStatus, Probes,
};
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::decision::{Decision, ExtraEvent, Field, Fields};
use moor_core::domain::session_state::SessionInput;
use moor_core::seam::clock::Ts;
use moor_core::seam::engine::SessionRef;
use moor_core::seam::proc::Deadline;
use moor_core::seam::surface::PaneRef;
use moor_core::view::LaunchFailure;
use moor_core::Warning;

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::boxlock::BoxLock;
use crate::ops::common::link::{self, Fate};
use crate::ops::common::provision::recorded_workdir;
use crate::ops::common::resume;
use crate::ops::common::transcript;
use crate::store::{BoxRow, Store, StoreError};

use launch::{launch_overdue, warn_launch_timed_out};
use stored::stored_hook;

/// Allowance for the hook file read and for the pane observation in one probe.
///
/// The multiplexer can take up to twice its deadline, so the pane read takes up to ten
/// seconds. The hook file read first looks up the Box (5 s) and then reads the file within
/// the allowance, each with 2.5 s of grace to stop the child, so one probe takes up to about 25 seconds.
pub(crate) const PROBE_DEADLINE: Deadline = Deadline(Duration::from_secs(5));

/// Whether the agent's pane is still there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PaneLiveness {
    /// The pane exists and its agent runs, or the pane could not be read this time.
    Live,
    /// The pane exists and shows the agent ended.
    Exited,
    /// There is no pane: it vanished, was never recorded, or the Box has no agent.
    Gone,
}

impl PaneLiveness {
    /// The name shown to users.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Exited => "exited",
            Self::Gone => "gone",
        }
    }
}

/// What one read of a Box's agent found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Reading {
    /// The agent's state; none when the Box has no live session.
    pub(crate) activity: Option<Activity>,
    /// Whether the pane is there.
    pub(crate) pane: PaneLiveness,
    /// Whether this reading may move a Working Box to Idle.
    pub(crate) may_quiet: bool,
    /// Whether a run of the Box is in flight.
    run_in_flight: bool,
    /// The live session this reading probed.
    pub(crate) session: Option<i64>,
    /// Why the agent never launched; the Box has no session then.
    pub(crate) launch_failure: Option<LaunchFailure>,
    /// The `box.launch_timeout`, in seconds, that a failed launch has outlasted.
    pub(crate) launch_overdue: Option<u64>,
    /// The agent's own session id, from the hook seen or the Box.
    agent_session: Option<String>,
    /// The pane text this reading observed; none when the pane gave no observation.
    pub(crate) pane_text: Option<String>,
    /// The agent's last message, when a hook reported it idle with one.
    said: Option<String>,
}

impl Reading {
    /// Whether the Box counts as busy: a run is in flight or the agent is busy.
    pub(crate) fn busy(&self) -> bool {
        self.run_in_flight || self.activity.as_ref().is_some_and(|a| a.state.busy())
    }

    /// Whether the agent waits on an unanswered prompt or a first-run screen.
    pub(crate) fn blocked(&self) -> bool {
        self.activity
            .as_ref()
            .is_some_and(|a| matches!(a.state, AgentState::NeedsInput { .. }))
    }

    /// No agent to read; a run in flight still keeps the Box busy.
    const fn none(run_in_flight: bool) -> Self {
        Self {
            activity: None,
            pane: PaneLiveness::Gone,
            may_quiet: !run_in_flight,
            run_in_flight,
            session: None,
            launch_failure: None,
            launch_overdue: None,
            agent_session: None,
            pane_text: None,
            said: None,
        }
    }

    /// No agent because its launch failed; the Box is not quiet for that until the failure has
    /// outlasted `box.launch_timeout`, which `overdue` names in seconds.
    const fn launch_failed(
        run_in_flight: bool,
        failure: LaunchFailure,
        overdue: Option<u64>,
    ) -> Self {
        Self {
            activity: None,
            pane: PaneLiveness::Gone,
            may_quiet: overdue.is_some() && !run_in_flight,
            run_in_flight,
            session: None,
            launch_failure: Some(failure),
            launch_overdue: overdue,
            agent_session: None,
            pane_text: None,
            said: None,
        }
    }
}

/// Read `row`'s agent once.
///
/// This is the only caller of `Engine::probe`. A Box with no live session, or whose machine has
/// no handle or workdir to reach, has no agent and is not probed; a run in flight still makes it busy. A fresh hook report is
/// cached; a dropped read leaves the cache as it was. A run in flight forces `Working` whatever
/// the probe says; the reading keeps the probe's source, as no source names a run. `deadline`
/// bounds each of the probe's reads; [`PROBE_DEADLINE`] unless the caller has less time.
///
/// # Errors
/// A store failure other than an undecodable cache row.
pub(crate) fn read(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    now: Ts,
    deadline: Deadline,
) -> Result<Reading, CommandError> {
    let in_flight = store.in_flight_run(row.id)?.is_some();
    let Some(session) = store.live_session(row.id)? else {
        return Ok(row.launch_failure.clone().map_or_else(
            || Reading::none(in_flight),
            |f| {
                let overdue = launch_overdue(cx, &f, now);
                Reading::launch_failed(in_flight, f, overdue)
            },
        ));
    };
    let Some(sref) = session_ref(row, session.session_ref, session.pane) else {
        return Ok(Reading::none(in_flight));
    };
    let cache = stored_hook(cx, store, row.id)?;
    let probes = cx.engine().probe(&sref, deadline).unwrap_or_else(|e| {
        warn(cx, format!("could not probe the agent: {e}"));
        Probes {
            hook: HookRead::TimedOut,
            pane: PaneRead::TimedOut,
        }
    });
    let mut agent_session = row.agent_session_id.clone();
    if let HookRead::Fresh(rec) = &probes.hook {
        store.put_hook(row.id, rec)?;
        doing::note_doing(store, row.id, rec, now)?;
        note_agent_session(store, row, rec.session_id.as_deref())?;
        agent_session = rec.session_id.clone().or(agent_session);
    }
    let pane_text = match &probes.pane {
        PaneRead::Fresh(obs) => Some(obs.tail.clone()),
        _ => None,
    };
    let first_run = first_run(cx, pane_text.as_deref());
    let mut activity = decide(&probes, cache.as_ref(), now, session.started_at, first_run);
    if in_flight {
        activity.state = AgentState::Working;
    }
    let may_quiet = !in_flight && may_apply_quiet(&activity, Some(&probes));
    let said = said::said(&activity, &probes.hook, cache.as_ref());
    Ok(Reading {
        pane: liveness(&probes.pane, &activity),
        activity: Some(activity),
        may_quiet,
        run_in_flight: in_flight,
        session: Some(session.id),
        launch_failure: None,
        launch_overdue: None,
        agent_session,
        pane_text,
        said,
    })
}

/// The name of the first-run screen the profile reads in the pane `tail`, for an agent that runs no hook
/// while one is up; any other agent's hooks report its prompts.
fn first_run(cx: &Cx, tail: Option<&str>) -> Option<&'static str> {
    tail.filter(|_| cx.engine().hooks_quiet_during_first_run())
        .and_then(|t| cx.engine().first_run_prompt(t))
        .map(|m| m.prompt.name())
}

/// Keep the agent's own session id on the Box; written only when it changed.
fn note_agent_session(store: &Store, row: &BoxRow, seen: Option<&str>) -> Result<(), CommandError> {
    let Some(id) = seen.filter(|id| row.agent_session_id.as_deref() != Some(id)) else {
        return Ok(());
    };
    let set = Fields {
        agent_session_id: Field::Set(id.to_owned()),
        ..Fields::default()
    };
    store.apply(Decision::new(row.id).set(set))?;
    Ok(())
}

/// Move the Box's state to match `reading`, and save the transcript of an agent first seen exited
/// and end its session.
///
/// The agent session id of the live session is registered with the multiplexer when it is not yet.
///
/// A Box that an op holds, or that has an intent set, is not moved.
///
/// A Working Box goes Idle only when the reading `may_quiet` and is not busy; an Idle Box
/// goes back to Working when the reading is busy. Nothing else decides quiet: a gone pane, an
/// idle status or a dropped hook read never idles a Box. If the row changed since the read,
/// the store refuses the step and the Box is left as the other writer set it.
///
/// # Errors
/// A store failure other than a refused step.
pub(crate) fn settle(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    reading: &Reading,
) -> Result<(), CommandError> {
    if turn::note_turn_done(store, row, reading)? && reading.pane != PaneLiveness::Exited {
        transcript::save_on_turn(cx, store, row)?;
    }
    if let (PaneLiveness::Exited, Some(probed)) = (reading.pane, reading.session) {
        if let Some(exit) = exited_agent(cx, store, row, reading, probed)? {
            transcript::save_on_exit(cx, store, row, probed)?;
            end_exited_session(cx, store, row, exit, probed)?;
        }
    }
    register_session(cx, store, row, reading)?;
    needs_input::note_needs_input(store, row, reading)?;
    let input = match (row.lifecycle == BoxLifecycle::Ready, row.session.is_quiet()) {
        (true, false) if !reading.busy() && reading.may_quiet => SessionInput::AgentQuiet,
        (true, true) if reading.busy() => SessionInput::AgentBusy,
        _ => return Ok(()),
    };
    // An op that owns the Box decides where the Box goes; hold the lock across the write.
    let Ok(_held) = BoxLock::acquire(
        cx.clock(),
        cx.state_dir()?,
        row.id,
        "settle",
        Duration::ZERO,
    ) else {
        return Ok(());
    };
    if row.intent.is_some() {
        return Ok(());
    }
    if let (SessionInput::AgentQuiet, Some(text)) = (&input, &reading.said) {
        let text = text.clone();
        store.apply(Decision::new(row.id).event(ExtraEvent::AgentSaid { text }))?;
    }
    let mut decision = Decision::new(row.id).session(input.clone());
    let timed_out = reading
        .launch_overdue
        .filter(|_| input == SessionInput::AgentQuiet);
    if let Some(after_secs) = timed_out {
        decision = decision.event(ExtraEvent::LaunchTimedOut { after_secs });
    }
    match store.apply(decision) {
        Ok(_) => {
            if let Some(after_secs) = timed_out {
                warn_launch_timed_out(cx, row, after_secs);
            }
            Ok(())
        }
        Err(StoreError::Illegal(_)) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// The exit status of the agent whose pane `reading` saw end, or `None` when the session goes
/// on: a new pane attached to the agent, the session is not the probed one, or another
/// operation holds the Box lock.
fn exited_agent(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    reading: &Reading,
    probed: i64,
) -> Result<Option<Option<i32>>, CommandError> {
    let Some(launch) = store.live_session(row.id)?.filter(|s| s.id == probed) else {
        return Ok(None);
    };
    let Ok(_held) = BoxLock::acquire(
        cx.clock(),
        cx.state_dir()?,
        row.id,
        "session end",
        Duration::ZERO,
    ) else {
        return Ok(None);
    };
    let pane_exit = match reading.activity.as_ref().map(|a| &a.state) {
        Some(AgentState::Exited { code }) => Some(*code),
        _ => None,
    };
    Ok(match link::recover(cx, store, row, &launch, pane_exit)? {
        Fate::Reattached => None,
        Fate::Ended(exit) => Some(exit),
    })
}

/// Ends the current session of an agent that ended with `exit`, once its transcript is saved.
///
/// Nothing is ended when the live session is not the probed one, or while another operation holds the Box lock, so the save is retried first.
fn end_exited_session(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    exit: Option<i32>,
    probed: i64,
) -> Result<(), CommandError> {
    if store.live_session(row.id)?.is_none_or(|s| s.id != probed) {
        return Ok(());
    }
    let Ok(_held) = BoxLock::acquire(
        cx.clock(),
        cx.state_dir()?,
        row.id,
        "session end",
        Duration::ZERO,
    ) else {
        return Ok(());
    };
    store.end_current_session(row.id, "exited", exit)?;
    Ok(())
}

/// Registers the agent session id `reading` found against the live session's pane.
fn register_session(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    reading: &Reading,
) -> Result<(), CommandError> {
    if reading.pane != PaneLiveness::Live
        || reading.agent_session.as_deref().is_none_or(str::is_empty)
    {
        return Ok(());
    }
    let Some(session) = store.live_session(row.id)? else {
        return Ok(());
    };
    let state = reading
        .activity
        .as_ref()
        .map_or(PaneStatus::Unknown, |a| resume::pane_status(&a.state));
    resume::ensure(
        cx,
        store,
        &row.name,
        &session,
        reading.agent_session.as_deref(),
        state,
    )
}

/// The session as the Engine needs it; none when the Box cannot be reached.
///
pub(crate) fn session_ref(row: &BoxRow, id: String, pane: Option<String>) -> Option<SessionRef> {
    let b = row.boxed.as_ref()?;
    Some(SessionRef {
        id,
        handle: b.handle.clone()?,
        workdir: recorded_workdir(b.workdir.as_deref()?)?,
        pane: pane.map(PaneRef::new),
        warnings: Vec::new(),
    })
}

/// Record a warning about the agent read.
fn warn(cx: &Cx, message: String) {
    cx.warn(Warning {
        code: "agent_read_degraded".to_owned(),
        message,
    });
}

/// Pane liveness from the raw pane read and the decided state.
const fn liveness(pane: &PaneRead, activity: &Activity) -> PaneLiveness {
    match pane {
        PaneRead::Gone | PaneRead::Absent => PaneLiveness::Gone,
        _ if matches!(activity.state, AgentState::Exited { .. }) => PaneLiveness::Exited,
        _ => PaneLiveness::Live,
    }
}

mod doing;
mod launch;
mod needs_input;
mod said;
mod stored;
mod turn;

#[cfg(test)]
mod tests;
