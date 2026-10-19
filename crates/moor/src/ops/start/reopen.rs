//! Opens the next Session of a Box whose last one ended, so an agent can launch into it.

use moor_core::domain::decision::{Decision, Field, Fields};
use moor_core::domain::session_state::SessionState;
use moor_core::BoxId;

use crate::error::CommandError;
use crate::store::Store;

/// Opens and starts a new Session when the Box's current one has ended, and says whether it did.
///
/// The new Session records `parent` as its parent and inherits the workspace and the agent session of the one it follows.
///
/// # Errors
/// A store failure.
pub(super) fn ended_session(
    store: &Store,
    id: BoxId,
    parent: Option<i64>,
) -> Result<bool, CommandError> {
    let ended = store
        .box_sessions(id)?
        .last()
        .is_some_and(|s| s.state == SessionState::Ended);
    if !ended {
        return Ok(false);
    }
    let prior = store.task(id)?;
    let next = store.open_child_session(id, parent)?;
    store.start_session(next.id)?;
    if let Some(row) = prior {
        let carry = Fields {
            workspace_id: row.workspace_id.map_or(Field::Keep, Field::Set),
            agent_session_id: row.agent_session_id.map_or(Field::Keep, Field::Set),
            ..Fields::default()
        };
        store.apply(Decision::new(id).set(carry))?;
    }
    Ok(true)
}
