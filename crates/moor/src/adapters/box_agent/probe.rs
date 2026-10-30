//! One probe: the hook file through the Driver, then the pane through the Surface.

use moor_core::activity::{HookKind, HookRead, PaneRead, Probes};
use moor_core::name::BoxPath;
use moor_core::seam::agent_adapter::AgentAdapter;
use moor_core::seam::driver::{BoxStep, Driver, DriverError, StepDone};
use moor_core::seam::engine::{EngineError, SessionRef};
use moor_core::seam::proc::Deadline;
use moor_core::seam::surface::{Surface, SurfaceError};

/// Where the agent's hook reports land, relative to the Box workdir.
pub(super) const HOOK_FILE: &str = ".moor/agent-state.json";

/// Reads the hook file once, then the pane once, each within `deadline`; the raw reads are
/// returned undecided. A turn the hooks report busy is real use of the Box, so it touches the
/// Box's lease; any other read only looks.
pub(super) fn probe(
    driver: &dyn Driver,
    surface: &dyn Surface,
    profile: &dyn AgentAdapter,
    s: &SessionRef,
    deadline: Deadline,
) -> Result<Probes, EngineError> {
    let hook = read_hook(driver, profile, s, deadline)?;
    if matches!(&hook, HookRead::Fresh(rec) if rec.kind == HookKind::Busy) {
        let _ = driver.touch(&s.handle);
    }
    let pane = read_pane(surface, profile, s, deadline);
    Ok(Probes { hook, pane })
}

/// The hook file read; a missing or unparsable file is `Absent`, a failed read is `TimedOut`.
fn read_hook(
    driver: &dyn Driver,
    profile: &dyn AgentAdapter,
    s: &SessionRef,
    deadline: Deadline,
) -> Result<HookRead, EngineError> {
    let path = BoxPath::under(s.workdir.as_str(), HOOK_FILE).map_err(|e| {
        EngineError::Box(DriverError::Failed {
            summary: e.to_string(),
        })
    })?;
    let read = [BoxStep::Peek(path)];
    match driver.batch(&s.handle, &[], &read, deadline).pop() {
        Some(Ok(StepDone::Peeked(Some(text)))) => Ok(profile
            .parse_hook(text.as_bytes())
            .map_or(HookRead::Absent, HookRead::Fresh)),
        Some(Ok(StepDone::Peeked(None))) => Ok(HookRead::Absent),
        _ => Ok(HookRead::TimedOut),
    }
}

/// The pane read; a missing pane is `Gone`, no pane at all is `Absent`, a failed read is `TimedOut`.
fn read_pane(
    surface: &dyn Surface,
    profile: &dyn AgentAdapter,
    s: &SessionRef,
    deadline: Deadline,
) -> PaneRead {
    let Some(pane) = &s.pane else {
        return PaneRead::Absent;
    };
    match surface.pane_obs(pane, deadline) {
        Ok(mut obs) => {
            obs.prompt = profile.pane_prompt(&obs.tail);
            PaneRead::Fresh(obs)
        }
        Err(SurfaceError::NotFound) => PaneRead::Gone,
        Err(_) => PaneRead::TimedOut,
    }
}

#[cfg(test)]
mod tests;
