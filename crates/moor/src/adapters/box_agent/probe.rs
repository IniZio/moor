//! One probe: the hook file through the Driver, then the pane through the Surface.

use moor_core::activity::{HookRead, PaneRead, Probes};
use moor_core::name::BoxPath;
use moor_core::seam::agent_profile::AgentProfile;
use moor_core::seam::driver::{Driver, DriverError};
use moor_core::seam::engine::{EngineError, SessionRef};
use moor_core::seam::proc::Deadline;
use moor_core::seam::surface::{Surface, SurfaceError};

/// Where the agent's hook reports land, relative to the Box workdir.
const HOOK_FILE: &str = ".moor/agent-state.json";

/// Reads the hook file once, then the pane once; the raw reads are returned undecided.
pub(super) fn probe(
    driver: &dyn Driver,
    surface: &dyn Surface,
    profile: &dyn AgentProfile,
    s: &SessionRef,
    deadline: Deadline,
) -> Result<Probes, EngineError> {
    let hook = read_hook(driver, profile, s)?;
    let pane = read_pane(surface, s, deadline);
    Ok(Probes { hook, pane })
}

/// The hook file read; a missing or unparsable file is `Absent`, a failed read is `TimedOut`.
fn read_hook(
    driver: &dyn Driver,
    profile: &dyn AgentProfile,
    s: &SessionRef,
) -> Result<HookRead, EngineError> {
    let path = BoxPath::under(s.workdir.as_str(), HOOK_FILE).map_err(|e| {
        EngineError::Box(DriverError::Failed {
            summary: e.to_string(),
        })
    })?;
    match driver.peek_file(&s.handle, &path, &s.worktree) {
        Ok(Some(text)) => Ok(profile
            .parse_hook(text.as_bytes())
            .map_or(HookRead::Absent, HookRead::Fresh)),
        Ok(None) => Ok(HookRead::Absent),
        Err(_) => Ok(HookRead::TimedOut),
    }
}

/// The pane read; a missing pane is `Gone`, no pane at all is `Absent`, a failed read is `TimedOut`.
fn read_pane(surface: &dyn Surface, s: &SessionRef, deadline: Deadline) -> PaneRead {
    let Some(pane) = &s.pane else {
        return PaneRead::Absent;
    };
    match surface.pane_obs(pane, deadline) {
        Ok(obs) => PaneRead::Fresh(obs),
        Err(SurfaceError::NotFound) => PaneRead::Gone,
        Err(_) => PaneRead::TimedOut,
    }
}

#[cfg(test)]
mod tests;
