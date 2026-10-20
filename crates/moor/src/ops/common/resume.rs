//! Tells the multiplexer how to resume a pane's agent session.

use moor_core::activity::{AgentState, PaneStatus};
use moor_core::seam::surface::{PaneRef, ResumeOutcome, ResumeReg};
use moor_core::{BoxName, Warning};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::{SessionRow, Store};

/// Agent label registered with the multiplexer.
const AGENT: &str = "claude";

/// The pane status that shows `state`.
pub(crate) const fn pane_status(state: &AgentState) -> PaneStatus {
    match state {
        AgentState::Working => PaneStatus::Working,
        AgentState::Idle => PaneStatus::Idle,
        AgentState::NeedsInput { .. } => PaneStatus::Blocked,
        _ => PaneStatus::Unknown,
    }
}

/// Registers how to resume session `agent` in `pane`, for Box `name`; true when registered.
///
/// A refusal or a failure is a warning, never an error.
pub(crate) fn report(cx: &Cx, name: &BoxName, pane: &str, agent: &str, state: PaneStatus) -> bool {
    let reg = ResumeReg {
        agent: AGENT.to_owned(),
        state,
        session_id: agent.to_owned(),
        box_name: name.clone(),
    };
    let message = match cx.surface().register_resume(&PaneRef::new(pane), &reg) {
        Ok(ResumeOutcome::Registered) => return true,
        Ok(ResumeOutcome::Rejected(code)) => {
            format!(
                "the multiplexer declined to register resume for {}: {code}",
                name.as_str()
            )
        }
        Err(e) => format!("could not register resume for {}: {e}", name.as_str()),
    };
    cx.warn(Warning {
        code: "resume_not_registered".to_owned(),
        message,
    });
    false
}

/// Registers the agent session `agent` against `session`'s pane unless it already is.
///
/// Only a registration the multiplexer accepted is kept, so a failed one is tried again.
///
/// # Errors
/// A store failure.
pub(crate) fn ensure(
    cx: &Cx,
    store: &Store,
    name: &BoxName,
    session: &SessionRow,
    agent: Option<&str>,
    state: PaneStatus,
) -> Result<(), CommandError> {
    let (Some(agent), Some(pane)) = (agent.filter(|a| !a.is_empty()), session.pane.as_deref())
    else {
        return Ok(());
    };
    if session.registered_agent_session.as_deref() == Some(agent) {
        return Ok(());
    }
    if report(cx, name, pane, agent, state) {
        store.set_registered_session(session.id, agent)?;
    }
    Ok(())
}
