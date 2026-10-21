//! Host Sessions (no Box): `open-host-session`, `update-host-session`, `end-host-session`.

use std::path::Path;

use moor_core::domain::session_state::SessionState;
use moor_core::name::SessionId;
use moor_core::request::{EndHostSessionReq, OpenHostSessionReq, UpdateHostSessionReq};
use moor_core::view::session::HostSessionValue;
use moor_core::ErrorKind;

use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::StoreError;

/// Run `open-host-session`.
///
/// # Errors
/// [`CommandError`] when the workdir is empty or not absolute, the parent is unknown or ended, or the store write fails.
pub(crate) fn open(cx: &Cx, req: &OpenHostSessionReq) -> Result<HostSessionValue, CommandError> {
    if req.workdir.is_empty() || !Path::new(&req.workdir).is_absolute() {
        return Err(CommandError::new(
            ErrorKind::Usage,
            format!("workdir must be an absolute path, got {:?}", req.workdir),
        ));
    }
    let session = cx
        .store()?
        .open_child_host_session(
            &req.workdir,
            req.agent.as_deref(),
            req.agent_session_id.as_deref(),
            req.parent.map(SessionId::get),
        )
        .map_err(refuse_invalid)?;
    Ok(HostSessionValue {
        session: SessionId::new(session),
        workdir: req.workdir.clone(),
        state: SessionState::Working.as_str().to_owned(),
    })
}

/// Run `update-host-session`.
///
/// # Errors
/// [`CommandError`] when no field is given, or the Session is unknown, Box-backed or ended.
pub(crate) fn update(
    cx: &Cx,
    req: &UpdateHostSessionReq,
) -> Result<HostSessionValue, CommandError> {
    if req.agent.is_none() && req.agent_session_id.is_none() {
        return Err(CommandError::new(
            ErrorKind::Usage,
            "pass an agent or an agent session id to update",
        ));
    }
    let row = cx
        .store()?
        .update_host_session(
            req.session.get(),
            req.agent.as_deref(),
            req.agent_session_id.as_deref(),
        )
        .map_err(refuse_invalid)?;
    Ok(value(SessionId::new(row.id), row.workdir, row.state))
}

/// Run `end-host-session`.
///
/// # Errors
/// [`CommandError`] when the Session is unknown, Box-backed or already ended.
pub(crate) fn end(cx: &Cx, req: &EndHostSessionReq) -> Result<HostSessionValue, CommandError> {
    let row = cx
        .store()?
        .end_host_session(req.session.get())
        .map_err(refuse_invalid)?;
    Ok(value(SessionId::new(row.id), row.workdir, row.state))
}

/// A request the store rejects as invalid, such as for a Session that is not a live host
/// Session, is a refusal.
pub(super) fn refuse_invalid(e: StoreError) -> CommandError {
    match e {
        StoreError::Invalid(_) => CommandError::new(ErrorKind::Refused, e),
        other => other.into(),
    }
}

/// Build the response value for a host Session.
fn value(session: SessionId, workdir: String, state: SessionState) -> HostSessionValue {
    HostSessionValue {
        session,
        workdir,
        state: state.as_str().to_owned(),
    }
}

#[cfg(test)]
mod tests;
