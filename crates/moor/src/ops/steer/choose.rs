//! What a steer sends, given what the agent shows.

use moor_core::activity::AgentState;
use moor_core::seam::engine::Steer;
use moor_core::{ErrorKind, SteerReq};

use crate::error::CommandError;

/// What to send for `req` given what the agent shows.
///
/// A blocked agent takes a menu choice only: `--menu N`, or a lone digit as the text.
pub(super) fn choose(
    req: &SteerReq,
    box_name: &str,
    state: Option<&AgentState>,
) -> Result<Steer, CommandError> {
    if req.interrupt {
        return Ok(Steer::Interrupt);
    }
    let waiting = match state {
        Some(AgentState::NeedsInput { question, .. }) => Some(question),
        _ => None,
    };
    match (req.menu, req.text.as_deref(), waiting) {
        (Some(n), _, Some(_)) => Ok(Steer::Menu(choice(n)?)),
        (Some(_), _, None) => Err(CommandError::new(
            ErrorKind::Refused,
            format!("the agent of {box_name} is not waiting at a menu"),
        )),
        (None, Some(t), None) => Ok(Steer::Text(t.to_owned())),
        (None, Some(t), Some(w)) => match t.parse::<u8>() {
            Ok(n) if t.len() == 1 => Ok(Steer::Menu(n)),
            _ => Err(CommandError::new(
                ErrorKind::Refused,
                format!(
                    "the agent of {box_name} is blocked (waiting for: {}); answer with `moor steer {box_name} --menu N` or open the Workspace with `moor start {box_name}`",
                    w.as_deref().unwrap_or("input")
                ),
            )),
        },
        (None, None, _) => Err(CommandError::new(ErrorKind::Usage, "nothing to send")),
    }
}

/// A menu number the pane keys can carry.
fn choice(n: u32) -> Result<u8, CommandError> {
    u8::try_from(n)
        .ok()
        .filter(|n| *n > 0)
        .ok_or_else(|| CommandError::new(ErrorKind::Usage, format!("no menu choice {n}")))
}
