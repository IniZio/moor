//! Agent places: repo-less Boxes the agent waits in before any task.
//!
//! A place is started with `moor start --place` by a user on the controller's `agent_places`
//! list. Until a task claims it, no list, event stream, sleep or auto-close sees it; each of those
//! readers asks [`BoxKind::is_idle_place`].

use moor_core::domain::place::{BoxKind, PlaceState};
use moor_core::user::UserId;
use moor_core::{ErrorKind, StartReq};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::Store;

/// What a start makes of its Box: a place when it asks for one, else a work Box.
pub(crate) const fn kind_of(req: &StartReq) -> BoxKind {
    if req.place {
        BoxKind::Place(PlaceState::Idle)
    } else {
        BoxKind::Work
    }
}

/// Refuses a place start that names more than a place has, or that a user off the list makes. A
/// start that asks for no place passes.
///
/// # Errors
/// Usage naming the first argument a place does not take; Refused for a user not on the list.
pub(super) fn check_start(
    cx: &Cx,
    store: &Store,
    req: &StartReq,
    user: UserId,
) -> Result<(), CommandError> {
    if !req.place {
        return Ok(());
    }
    let named = [
        ("a repo", req.repo.is_some()),
        ("other repos", req.also.is_some()),
        ("a prompt", req.prompt.is_some()),
        ("an issue", req.issue.is_some()),
        ("a base", req.base.is_some()),
        ("a config", req.config.is_some()),
        ("gates", req.gates.is_some()),
        ("resume", req.resume),
        ("no agent", req.no_agent),
    ];
    if let Some((what, _)) = named.iter().find(|(_, given)| *given) {
        return Err(CommandError::new(
            ErrorKind::Usage,
            format!("a place takes {what} from no one: it starts with no repo and no prompt"),
        ));
    }
    let listed = store
        .email_of(user)?
        .is_some_and(|email| cx.env().agent_place_user(&email));
    if !listed {
        return Err(CommandError::new(
            ErrorKind::Refused,
            "you are not on this controller's agent_places list",
        ));
    }
    Ok(())
}
