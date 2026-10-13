//! The one read of a Task's agent: probe once, decide, keep the hook cache.

use std::path::PathBuf;
use std::time::Duration;

use moor_core::activity::{
    decide, may_apply_quiet, Activity, AgentState, HookRead, PaneRead, Probes,
};
use moor_core::seam::clock::Ts;
use moor_core::seam::engine::SessionRef;
use moor_core::seam::proc::Deadline;
use moor_core::Warning;

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::provision::recorded_workdir;
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
        }
    }

    /// No agent to read; a run in flight still keeps the Task busy.
    const fn none(run_in_flight: bool) -> Self {
        Self {
            activity: None,
            pane: PaneLiveness::Gone,
            may_quiet: !run_in_flight,
            run_in_flight,
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
        return Ok(Reading::none(in_flight));
    };
    let Some(sref) = session_ref(row, session.session_ref) else {
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
    })
}

/// The session as the Engine needs it; none when the Box cannot be reached.
///
/// No pane is stored for a Task yet, so the reference carries none.
fn session_ref(row: &TaskRow, id: String) -> Option<SessionRef> {
    let b = row.boxed.as_ref()?;
    Some(SessionRef {
        id,
        handle: b.handle.clone()?,
        workdir: recorded_workdir(b.workdir.as_deref()?)?,
        worktree: PathBuf::from(row.worktree.as_deref()?),
        pane: None,
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
