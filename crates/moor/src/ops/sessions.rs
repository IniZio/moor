//! `sessions`: every live Session, or the live children of one.

use moor_core::request::SessionsReq;
use moor_core::view::session::{SessionListItem, SessionsValue};
use moor_core::{ErrorKind, SessionId};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::authz;

/// Run `sessions`. Pure store reads: no reconcile, no Box lock, no driver or surface call.
///
/// # Errors
/// [`CommandError`] when `parent` is unknown or a store read fails.
pub(crate) fn run(cx: &Cx, req: &SessionsReq) -> Result<SessionsValue, CommandError> {
    let store = cx.store()?;
    let rows = match req.parent {
        None => store.live_sessions()?,
        Some(parent) => {
            let id = parent.get();
            store
                .session_row(id)?
                .ok_or_else(|| CommandError::new(ErrorKind::Failed, format!("no Session {id}")))?;
            store
                .session_children(id)?
                .into_iter()
                .filter(|r| !r.is_ended())
                .collect()
        }
    };
    let mut sessions = Vec::new();
    for r in rows {
        if !authz::may_see_session(cx, store, &r)? {
            continue;
        }
        sessions.push(SessionListItem {
            session: SessionId::new(r.id),
            box_name: r.box_name.map(|t| t.as_str().to_owned()),
            box_id: r.box_id,
            parent: r.parent.map(SessionId::new),
            state: r.state.as_str().to_owned(),
            workdir: r.workdir,
            ordinal: r.ordinal,
        });
    }
    Ok(SessionsValue { sessions })
}

#[cfg(test)]
mod tests;
