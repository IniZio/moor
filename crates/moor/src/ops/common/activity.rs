//! The one read of a Task's agent: probe once, decide, keep the hook cache.

use std::path::PathBuf;
use std::time::Duration;

use moor_core::activity::{
    decide, may_apply_quiet, Activity, AgentState, HookRead, PaneRead, Probes,
};
use moor_core::domain::decision::{Decision, Field, Fields};
use moor_core::domain::task::{TaskInput, TaskState};
use moor_core::seam::clock::Ts;
use moor_core::seam::engine::SessionRef;
use moor_core::seam::proc::Deadline;
use moor_core::seam::surface::PaneRef;
use moor_core::view::LaunchFailure;
use moor_core::Warning;

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::provision::recorded_workdir;
use crate::ops::common::transcript;
use crate::store::{Store, StoreError, TaskRow};

/// Allowance for the pane observation in one probe.
///
/// The multiplexer can take up to twice its deadline, so the pane read takes up to ten
/// seconds. The hook file read first looks up the Box (5 s) and then reads the file (5 s),
/// each with 2.5 s of grace to stop the child, so one probe takes up to about 25 seconds.
const PROBE_DEADLINE: Duration = Duration::from_secs(5);

/// Whether the agent's pane is still there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PaneLiveness {
    /// The pane exists and its agent runs, or the pane could not be read this time.
    Live,
    /// The pane exists and shows the agent ended.
    Exited,
    /// There is no pane: it vanished, was never recorded, or the Task has no agent.
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

/// What one read of a Task's agent found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Reading {
    /// The agent's state; none when the Task has no live session.
    pub(crate) activity: Option<Activity>,
    /// Whether the pane is there.
    pub(crate) pane: PaneLiveness,
    /// Whether this reading may move a Working Task to Idle.
    pub(crate) may_quiet: bool,
    /// Whether a run of the Task is in flight.
    run_in_flight: bool,
    /// Why the agent never launched; the Task has no session then.
    pub(crate) launch_failure: Option<LaunchFailure>,
}

impl Reading {
    /// Whether the Task counts as busy: a run is in flight or the agent is busy.
    pub(crate) fn busy(&self) -> bool {
        self.run_in_flight || self.activity.as_ref().is_some_and(|a| a.state.busy())
    }

    /// A reading of the given parts, for tests.
    #[cfg(test)]
    pub(crate) const fn new(
        activity: Option<Activity>,
        pane: PaneLiveness,
        may_quiet: bool,
        run_in_flight: bool,
    ) -> Self {
        Self {
            activity,
            pane,
            may_quiet,
            run_in_flight,
            launch_failure: None,
        }
    }

    /// No agent to read; a run in flight still keeps the Task busy.
    const fn none(run_in_flight: bool) -> Self {
        Self {
            activity: None,
            pane: PaneLiveness::Gone,
            may_quiet: !run_in_flight,
            run_in_flight,
            launch_failure: None,
        }
    }

    /// No agent because its launch failed; the Task is never quiet for that.
    const fn launch_failed(run_in_flight: bool, failure: LaunchFailure) -> Self {
        Self {
            activity: None,
            pane: PaneLiveness::Gone,
            may_quiet: false,
            run_in_flight,
            launch_failure: Some(failure),
        }
    }
}

/// Read `row`'s agent once.
///
/// This is the only caller of `Engine::probe`. A Task with no live session, or whose Box has
/// no handle or workdir to reach, has no agent and is not probed; a run in flight still makes it busy. A fresh hook report is
/// cached; a dropped read leaves the cache as it was. A run in flight forces `Working` whatever
/// the probe says; the reading keeps the probe's source, as no source names a run.
///
/// # Errors
/// A store failure other than an undecodable cache row.
pub(crate) fn read(
    cx: &Cx,
    store: &Store,
    row: &TaskRow,
    now: Ts,
) -> Result<Reading, CommandError> {
    let in_flight = store
        .in_flight_run(row.id, &|pid| cx.proc().alive(pid))?
        .is_some();
    let Some(session) = store.live_session(row.id)? else {
        return Ok(row.launch_failure.clone().map_or_else(
            || Reading::none(in_flight),
            |f| Reading::launch_failed(in_flight, f),
        ));
    };
    let Some(sref) = session_ref(row, session.session_ref, session.pane) else {
        return Ok(Reading::none(in_flight));
    };
    let cache = match store.hook(row.id) {
        Ok(c) => c,
        Err(StoreError::Invalid(why) | StoreError::Corrupt(why)) => {
            warn(cx, format!("ignored the stored hook report: {why}"));
            None
        }
        Err(StoreError::Json(e)) => {
            warn(cx, format!("ignored the stored hook report: {e}"));
            None
        }
        Err(e) => return Err(e.into()),
    };
    let probes = cx
        .engine()
        .probe(&sref, Deadline(PROBE_DEADLINE))
        .unwrap_or_else(|e| {
            warn(cx, format!("could not probe the agent: {e}"));
            Probes {
                hook: HookRead::TimedOut,
                pane: PaneRead::TimedOut,
            }
        });
    if let HookRead::Fresh(rec) = &probes.hook {
        store.put_hook(row.id, rec)?;
        note_agent_session(store, row, rec.session_id.as_deref())?;
    }
    let mut activity = decide(&probes, cache.as_ref(), now, session.started_at);
    if in_flight {
        activity.state = AgentState::Working;
    }
    let may_quiet = !in_flight && may_apply_quiet(&activity, Some(&probes));
    Ok(Reading {
        pane: liveness(&probes.pane, &activity),
        activity: Some(activity),
        may_quiet,
        run_in_flight: in_flight,
        launch_failure: None,
    })
}

/// Keep the agent's own session id on the Task; written only when it changed.
fn note_agent_session(
    store: &Store,
    row: &TaskRow,
    seen: Option<&str>,
) -> Result<(), CommandError> {
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

/// Move the Task's state to match `reading`, and save the transcript of an agent first seen exited.
///
/// A Working Task goes Idle only when the reading `may_quiet` and is not busy; an Idle Task
/// goes back to Working when the reading is busy. Nothing else decides quiet: a gone pane, an
/// idle status or a dropped hook read never idles a Task. If the row changed since the read,
/// the store refuses the step and the Task is left as the other writer set it.
///
/// # Errors
/// A store failure other than a refused step.
pub(crate) fn settle(
    cx: &Cx,
    store: &Store,
    row: &TaskRow,
    reading: &Reading,
) -> Result<(), CommandError> {
    if reading.pane == PaneLiveness::Exited {
        transcript::save_on_exit(cx, store, row)?;
    }
    let input = match row.state {
        TaskState::Working if !reading.busy() && reading.may_quiet => TaskInput::AgentQuiet,
        TaskState::Idle if reading.busy() => TaskInput::AgentBusy,
        _ => return Ok(()),
    };
    match store.apply(Decision::new(row.id).task_input(input)) {
        Ok(_) | Err(StoreError::Illegal(_)) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// The session as the Engine needs it; none when the Box cannot be reached.
///
pub(crate) fn session_ref(row: &TaskRow, id: String, pane: Option<String>) -> Option<SessionRef> {
    let b = row.boxed.as_ref()?;
    Some(SessionRef {
        id,
        handle: b.handle.clone()?,
        workdir: recorded_workdir(b.workdir.as_deref()?)?,
        worktree: PathBuf::from(row.worktree.as_deref()?),
        pane: pane.map(PaneRef::new),
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

#[cfg(test)]
mod tests;
