//! The one read of a Box's agent: probe once, decide, keep the hook cache.

use std::time::Duration;

use moor_core::activity::{decide, may_apply_quiet, Activity, AgentState, Probe, Term};
use moor_core::domain::decision::{Decision, Field, Fields};
use moor_core::seam::clock::Ts;
use moor_core::seam::engine::SessionRef;
use moor_core::seam::proc::Deadline;
use moor_core::view::LaunchFailure;
use moor_core::{BoxHandle, BoxPath, Warning};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::boxlock::BoxLock;
use crate::ops::common::link::Fate;
use crate::ops::common::provision::recorded_workdir;
use crate::ops::common::transcript;
use crate::store::{BoxRow, SessionRow, Store};

use launch::launch_overdue;

/// Allowance for one probe: the Box lookup (5 s) then one Box command, each with 2.5 s of grace
/// to stop the child.
pub(crate) const PROBE_DEADLINE: Deadline = Deadline(Duration::from_secs(5));

/// Whether the agent's terminal is still there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PaneLiveness {
    /// The agent runs, or could not be read this time.
    Live,
    /// The agent ended.
    Exited,
    /// There is no agent: none was launched, or the Box has no session.
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
    /// The pane of that session this reading probed.
    pub(crate) probed_pane: Option<String>,
    /// Why the agent never launched; the Box has no session then.
    pub(crate) launch_failure: Option<LaunchFailure>,
    /// The `box.launch_timeout`, in seconds, that a failed launch has outlasted.
    pub(crate) launch_overdue: Option<u64>,
    /// The agent's own session id, from the hook seen or the Box.
    agent_session: Option<String>,
    /// The agent's screen this reading observed; none when it was not read.
    pub(crate) pane_text: Option<String>,
    /// The agent's last message, when it is idle and its hooks left one.
    said: Option<String>,
    /// Whether the read failed.
    pub(crate) read_failed: bool,
    /// The agent's terminal, when read.
    pub(crate) term: Option<Term>,
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
            probed_pane: None,
            launch_failure: None,
            launch_overdue: None,
            agent_session: None,
            pane_text: None,
            said: None,
            read_failed: false,
            term: None,
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
            probed_pane: None,
            launch_failure: Some(failure),
            launch_overdue: overdue,
            agent_session: None,
            pane_text: None,
            said: None,
            read_failed: false,
            term: None,
        }
    }
}

/// Read `row`'s agent once.
///
/// The only caller of `Engine::probe`; a Box that is asleep or has no live session, handle or
/// workdir is not probed. A run in flight forces `Working`.
///
/// # Errors
/// A store failure.
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
    let probed_pane = session.pane.clone();
    let Some(sref) = session_ref(row, &session) else {
        return Ok(Reading::none(in_flight));
    };
    let asleep = row.asleep();
    let probe = probe(cx, &sref, asleep, deadline);
    let seen = probe.note().and_then(|n| n.session_id.as_deref());
    note_agent_session(store, row, seen)?;
    let agent_session = seen
        .map(str::to_owned)
        .or_else(|| row.agent_session_id.clone());
    if let Some(term) = probe.term() {
        doing::note_doing(store, row.id, term, now)?;
    }
    if probe != Probe::TimedOut {
        store.note_agent_read(session.id, now)?;
    }
    let pane_text = probe.screen().map(str::to_owned);
    let first_run = first_run(cx, pane_text.as_deref());
    let mut activity = decide(&probe, now, session.started_at, first_run);
    activity = stall::end_stale_wait(cx, &sref, &probe, activity, now, deadline);
    let pane = if asleep {
        PaneLiveness::Gone
    } else {
        liveness(&activity)
    };
    if in_flight {
        activity.state = AgentState::Working;
    }
    let may_quiet = !asleep && !in_flight && may_apply_quiet(&activity, Some(&probe));
    let said = said::said(&activity, &probe);
    Ok(Reading {
        pane,
        activity: Some(activity),
        may_quiet,
        run_in_flight: in_flight,
        session: Some(session.id),
        probed_pane,
        launch_failure: None,
        launch_overdue: None,
        agent_session,
        pane_text,
        said,
        read_failed: !asleep && matches!(probe, Probe::TimedOut),
        term: probe.term().cloned(),
    })
}

/// What the agent `sref` answers. A suspended guest answers nothing, and asking ties up its host
/// until the call times out, so an `asleep` Box answers nothing.
fn probe(cx: &Cx, sref: &SessionRef, asleep: bool, deadline: Deadline) -> Probe {
    if asleep {
        return Probe::TimedOut;
    }
    let probe = cx.engine().probe(sref, deadline);
    if probe == Probe::TimedOut {
        warn(cx, "could not read the agent".to_owned());
    }
    probe
}

/// The name of the first-run screen the profile reads on the agent's screen `tail`.
fn first_run(cx: &Cx, tail: Option<&str>) -> Option<&'static str> {
    tail.and_then(|t| cx.engine().first_run_prompt(t))
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

/// Move the Box's state to match `reading`, copy the transcript at each turn end, save the
/// transcript of an agent seen exited and end its session, and return the fate it answered.
///
/// An asleep Box is not moved: nothing was read from it, and a wake asks after its agent. A Box
/// that an op holds, or that has an intent set, is not moved. A Working Box goes Idle
/// only when the reading `may_quiet` and is not busy; an Idle Box goes back to Working when the
/// reading shows work. If the row changed since the read, the store refuses the step.
///
/// # Errors
/// A store failure other than a refused step.
pub(crate) fn settle(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    reading: &Reading,
) -> Result<Option<Fate>, CommandError> {
    if row.asleep() {
        return Ok(None);
    }
    let turn_done = turn::note_turn_done(store, row, reading, cx.clock().now())?;
    let fate = if let (PaneLiveness::Exited, Some(probed)) = (reading.pane, reading.session) {
        let exit = match reading.activity.as_ref().map(|a| &a.state) {
            Some(AgentState::Exited { code }) => *code,
            _ => None,
        };
        transcript::save_on_exit(cx, store, row, probed)?;
        end_exited_session(cx, store, row, exit, probed)?;
        Some(Fate::Ended(exit))
    } else {
        None
    };
    needs_input::note_needs_input(store, row, reading)?;
    stall::note_stall(store, row, reading, cx.clock().now())?;
    let went_quiet = quiet::step(cx, store, row, reading)?;
    if (turn_done || went_quiet) && reading.pane != PaneLiveness::Exited {
        transcript::save_on_turn(cx, store, row)?;
    }
    Ok(fate)
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

/// The agent `launch` as the Engine needs it; none when the Box cannot be reached.
pub(crate) fn session_ref(row: &BoxRow, launch: &SessionRow) -> Option<SessionRef> {
    let (handle, workdir) = box_place(row)?;
    Some(SessionRef {
        id: launch.session_ref.clone(),
        handle,
        workdir,
    })
}

/// The Box machine of `row` and its recorded workdir; none when the Box cannot be reached.
pub(crate) fn box_place(row: &BoxRow) -> Option<(BoxHandle, BoxPath)> {
    let b = row.boxed.as_ref()?;
    Some((b.handle.clone()?, recorded_workdir(b.workdir.as_deref()?)?))
}

/// Record a warning about the agent read.
fn warn(cx: &Cx, message: String) {
    cx.warn(Warning {
        code: "agent_read_degraded".to_owned(),
        message,
    });
}

/// Liveness from the decided state.
const fn liveness(activity: &Activity) -> PaneLiveness {
    if matches!(activity.state, AgentState::Exited { .. }) {
        PaneLiveness::Exited
    } else {
        PaneLiveness::Live
    }
}

mod doing;
mod launch;
mod needs_input;
mod quiet;
mod said;
mod stall;
mod turn;

#[cfg(test)]
mod tests;
