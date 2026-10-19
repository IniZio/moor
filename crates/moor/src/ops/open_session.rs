//! `open-session`: start a fresh agent Session on a Ready Box, seeded with the caller's text.

use moor_core::domain::boxes::BoxState;
use moor_core::domain::session_state::SessionState;
use moor_core::request::OpenSessionReq;
use moor_core::view::session::OpenSessionValue;
use moor_core::{BoxName, ErrorKind, SessionId};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::boxlock::{self, BoxLock};
use crate::ops::common::cred_sweep;
use crate::ops::host_session::refuse_invalid;
use crate::ops::start::begin_seeded;
use crate::store::{Store, TaskRow};

/// Run `open-session`.
///
/// # Errors
/// [`CommandError`] when the Task is unknown, its Box is not Ready, a Session is still live, or
/// the agent could not be launched.
pub(crate) fn run(cx: &Cx, req: &OpenSessionReq) -> Result<OpenSessionValue, CommandError> {
    let store = cx.store()?;
    let name = req.task.as_str();
    let parsed = BoxName::parse_allow_bare(name)
        .map_err(|e| CommandError::new(ErrorKind::Usage, e.to_string()))?;
    let id = store
        .open_task_by_name(&parsed)?
        .ok_or_else(|| CommandError::new(ErrorKind::Failed, format!("no open task {name}")))?
        .id;
    if let Some(parent) = req.parent {
        store.check_parent(parent.get()).map_err(refuse_invalid)?;
    }
    let reload = || {
        store
            .task(id)?
            .ok_or_else(|| CommandError::new(ErrorKind::Failed, format!("no task {name}")))
    };
    check(store, &reload()?, name)?;
    let _held = BoxLock::acquire(
        cx.clock(),
        cx.state_dir()?,
        id,
        "open-session",
        boxlock::WAIT,
    )?;
    let row = reload()?;
    check(store, &row, name)?;
    let next = store.open_child_session(id, req.parent.map(SessionId::get))?;
    store.start_session(next.id)?;
    cred_sweep::sweep(cx, &row);
    let view = begin_seeded(cx, store, &row, req.seed.clone())?;
    if !view.launched {
        store.end_current_session(id, "launch_failed", None)?;
        let message = view.reason.map_or_else(
            || format!("could not launch an agent in Box {name}"),
            |f| format!("{}: {}", f.step, f.message),
        );
        return Err(CommandError::new(ErrorKind::Failed, message));
    }
    Ok(OpenSessionValue {
        task: name.to_owned(),
        session: next.id,
        ordinal: next.ordinal,
        pane: store.live_session(id)?.and_then(|s| s.pane),
    })
}

/// Refuses a Box that is not Ready or whose current Session has not ended.
fn check(store: &Store, row: &TaskRow, name: &str) -> Result<(), CommandError> {
    if !row
        .boxed
        .as_ref()
        .is_some_and(|b| b.state == BoxState::Ready)
    {
        let err = CommandError::new(ErrorKind::Refused, format!("Box {name} is not Ready"));
        let asleep = row
            .boxed
            .as_ref()
            .is_some_and(|b| b.state == BoxState::Asleep);
        return Err(if asleep {
            err.with_hint(format!("run `moor wake {name}` first"))
        } else {
            err
        });
    }
    let live = store
        .box_sessions(row.id)?
        .pop()
        .is_some_and(|s| s.state != SessionState::Ended);
    if live {
        return Err(CommandError::new(
            ErrorKind::Refused,
            format!("Box {name} already has a live Session"),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
