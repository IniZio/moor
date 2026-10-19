//! `open-host-session`: register a Session that runs on the host, with no Box.

use std::path::Path;

use moor_core::domain::session_state::SessionState;
use moor_core::name::SessionId;
use moor_core::request::OpenHostSessionReq;
use moor_core::view::session::HostSessionValue;
use moor_core::ErrorKind;

use crate::cx::Cx;
use crate::error::CommandError;

/// Run `open-host-session`.
///
/// # Errors
/// [`CommandError`] when the workdir is empty or not absolute, or the store write fails.
pub(crate) fn open(cx: &Cx, req: &OpenHostSessionReq) -> Result<HostSessionValue, CommandError> {
    if req.workdir.is_empty() || !Path::new(&req.workdir).is_absolute() {
        return Err(CommandError::new(
            ErrorKind::Usage,
            format!("workdir must be an absolute path, got {:?}", req.workdir),
        ));
    }
    let session = cx.store()?.open_host_session(
        &req.workdir,
        req.agent.as_deref(),
        req.agent_session_id.as_deref(),
    )?;
    Ok(HostSessionValue {
        session: SessionId::new(session),
        workdir: req.workdir.clone(),
        state: SessionState::Working.as_str().to_owned(),
    })
}

#[cfg(test)]
mod tests;
