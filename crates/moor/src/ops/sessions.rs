//! `sessions`: every live Session, or the live children of one.

use moor_core::request::SessionsReq;
use moor_core::view::session::{SessionListItem, SessionsValue};
use moor_core::{ErrorKind, SessionId};

use crate::cx::Cx;
use crate::error::CommandError;

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
    let sessions = rows
        .into_iter()
        .map(|r| SessionListItem {
            session: SessionId::new(r.id),
            box_name: r.task.map(|t| t.as_str().to_owned()),
            parent: r.parent.map(SessionId::new),
            state: r.state.as_str().to_owned(),
            workdir: r.workdir,
            ordinal: r.ordinal,
        })
        .collect();
    Ok(SessionsValue { sessions })
}

#[cfg(test)]
mod tests;
