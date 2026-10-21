//! `show-session`: one Session by id, with its child Sessions.

use moor_core::request::ShowSessionReq;
use moor_core::view::session::{SessionChildView, SessionView};
use moor_core::{ErrorKind, SessionId};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::authz;

/// Run `show-session`. Read-only: no reconcile and no Box lock, so it works for host and Box Sessions.
///
/// # Errors
/// [`CommandError`] when the Session is unknown or a store read fails.
pub(crate) fn run(cx: &Cx, req: &ShowSessionReq) -> Result<SessionView, CommandError> {
    let store = cx.store()?;
    let id = req.session.get();
    let row = store
        .session_row(id)?
        .ok_or_else(|| CommandError::new(ErrorKind::Failed, format!("no Session {id}")))?;
    let mut children = Vec::new();
    for c in store.session_children(id)? {
        if let Some(id) = c.box_id {
            if !authz::may_see(cx, store, id)? {
                continue;
            }
        }
        children.push(SessionChildView {
            session: SessionId::new(c.id),
            box_name: c.box_name.map(|t| t.as_str().to_owned()),
            box_id: c.box_id,
            state: c.state.as_str().to_owned(),
        });
    }
    Ok(SessionView {
        session: SessionId::new(row.id),
        box_name: row.box_name.map(|t| t.as_str().to_owned()),
        box_id: row.box_id,
        ordinal: row.ordinal,
        parent: row.parent.map(SessionId::new),
        workdir: row.workdir,
        agent: row.agent,
        agent_session_id: row.agent_session_id,
        state: row.state.as_str().to_owned(),
        started_at: row.started_at,
        ended_at: row.ended_at,
        children,
    })
}

#[cfg(test)]
mod tests;
