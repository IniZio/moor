//! Tells the multiplexer how to resume a pane's agent session.

use moor_core::activity::{AgentState, PaneStatus};
use moor_core::seam::surface::{PaneRef, ResumeOutcome, ResumeReg};
use moor_core::{TaskName, Warning};

use crate::cx::Cx;

/// Agent label registered with the multiplexer.
const AGENT: &str = "claude";

/// The pane status that shows `state`.
pub(crate) const fn pane_status(state: &AgentState) -> PaneStatus {
    match state {
        AgentState::Working => PaneStatus::Working,
        AgentState::Idle => PaneStatus::Idle,
        AgentState::Blocked { .. } => PaneStatus::Blocked,
        _ => PaneStatus::Unknown,
    }
}

/// Registers how to resume session `session_id` in `pane`, for Task `task`.
///
/// Nothing is registered without an id. A refusal or a failure is a warning, never an error.
pub(crate) fn register(
    cx: &Cx,
    task: &TaskName,
    pane: &str,
    session_id: Option<&str>,
    state: PaneStatus,
) {
    let Some(session_id) = session_id.filter(|s| !s.is_empty()) else {
        return;
    };
    let reg = ResumeReg {
        agent: AGENT.to_owned(),
        state,
        session_id: session_id.to_owned(),
        task: task.clone(),
    };
    let message = match cx.surface().register_resume(&PaneRef::new(pane), &reg) {
        Ok(ResumeOutcome::Registered) => return,
        Ok(ResumeOutcome::Rejected(code)) => {
            format!(
                "the multiplexer declined to register resume for {}: {code}",
                task.as_str()
            )
        }
        Err(e) => format!("could not register resume for {}: {e}", task.as_str()),
    };
    cx.warn(Warning {
        code: "resume_not_registered".to_owned(),
        message,
    });
}
