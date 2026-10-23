//! Host Sessions (no Box): `open-host-session`, `update-host-session`, `end-host-session`.

use std::path::{Path, PathBuf};

use moor_core::domain::session_state::SessionState;
use moor_core::name::SessionId;
use moor_core::request::{EndHostSessionReq, OpenHostSessionReq, UpdateHostSessionReq};
use moor_core::user::{UserId, Viewer};
use moor_core::view::session::HostSessionValue;
use moor_core::ErrorKind;

use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::host::{HostWorkdir, NewHost};
use crate::store::StoreError;

/// Most bytes in a host Session key.
const KEY_MAX: usize = 256;

/// The refusal for a workdir from anyone but the operator.
pub(crate) const WORKDIR_OPERATOR_ONLY: &str =
    "only the operator chooses a host Session's workdir; send none and the controller gives the Session its own";

/// Run `open-host-session`. The operator names the workdir. Anyone else names none and gets
/// `<state>/users/<user>/sessions/<session>`, created owner-only.
///
/// # Errors
/// [`CommandError`] when the operator's workdir is missing or not absolute, another caller
/// names one or has no user, the parent is unknown or ended, the directory cannot be made, or
/// the store write fails.
pub(crate) fn open(cx: &Cx, req: &OpenHostSessionReq) -> Result<HostSessionValue, CommandError> {
    if req
        .key
        .as_ref()
        .is_some_and(|k| k.is_empty() || k.len() > KEY_MAX)
    {
        return Err(CommandError::new(
            ErrorKind::Usage,
            format!("a host Session key needs 1 to {KEY_MAX} bytes"),
        ));
    }
    let (user, base) = placement(cx, req)?;
    let workdir = match (&base, req.workdir.as_deref()) {
        (Some(base), _) => HostWorkdir::Under(base),
        (None, dir) => HostWorkdir::At(dir.unwrap_or_default()),
    };
    let (session, workdir, created) = cx
        .store()?
        .open_child_host_session(&NewHost {
            workdir,
            agent: req.agent.as_deref(),
            agent_session_id: req.agent_session_id.as_deref(),
            parent: req.parent.map(SessionId::get),
            user,
            key: req.key.as_deref(),
        })
        .map_err(refuse_invalid)?;
    if base.is_some() && created {
        if let Err(e) = own_dir::create(cx.state_dir()?, Path::new(&workdir)) {
            let _ended = cx.store()?.end_host_session(session);
            return Err(CommandError::new(
                ErrorKind::Failed,
                format!("create {workdir}: {e}"),
            ));
        }
    }
    Ok(HostSessionValue {
        session: SessionId::new(session),
        workdir,
        state: SessionState::Working.as_str().to_owned(),
    })
}

/// The user a host Session belongs to and, for anyone but the operator, the directory its
/// own workdir goes in.
fn placement(
    cx: &Cx,
    req: &OpenHostSessionReq,
) -> Result<(Option<UserId>, Option<PathBuf>), CommandError> {
    Ok(match (cx.viewer(), req.workdir.as_deref()) {
        (Viewer::Operator, Some(dir)) if Path::new(dir).is_absolute() => (None, None),
        (Viewer::Operator, dir) => {
            return Err(CommandError::new(
                ErrorKind::Usage,
                format!("workdir must be an absolute path, got {dir:?}"),
            ))
        }
        (Viewer::User(_), Some(_)) => {
            return Err(CommandError::new(ErrorKind::Refused, WORKDIR_OPERATOR_ONLY))
        }
        (Viewer::User(user), None) => {
            let base = cx
                .state_dir()?
                .join("users")
                .join(user.get().to_string())
                .join("sessions");
            (Some(user), Some(base))
        }
        (Viewer::Nobody, _) => {
            return Err(CommandError::new(
                ErrorKind::Refused,
                "log in first: moor user enroll",
            ))
        }
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

mod own_dir;

#[cfg(test)]
mod tests;
