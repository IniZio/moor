//! `end-session`: end the Box's live agent Session and keep the Box open for a later one.

use moor_core::decide::transcript::Trigger;
use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::boxes::BoxState;
use moor_core::domain::session_state::SessionState;
use moor_core::request::EndSessionReq;
use moor_core::view::session::EndSessionValue;
use moor_core::{BoxName, ErrorKind};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::agent_stop::stop_agent;
use crate::ops::common::boxlock::{self, BoxLock};
use crate::ops::common::transcript;
use crate::ops::common::wake::wake_and_revive;
use crate::store::{BoxRow, Store};

/// Run `end-session`.
///
/// # Errors
/// [`CommandError`] when the Task is unknown, the Box has no live Session, or the agent pane
/// could not be closed.
pub(crate) fn run(cx: &Cx, req: &EndSessionReq) -> Result<EndSessionValue, CommandError> {
    let store = cx.store()?;
    let name = req.box_name.as_str();
    let parsed = BoxName::parse_allow_bare(name)
        .map_err(|e| CommandError::new(ErrorKind::Usage, e.to_string()))?;
    let id = store
        .open_box_by_name(&parsed)?
        .ok_or_else(|| CommandError::new(ErrorKind::Failed, format!("no open task {name}")))?
        .id;
    let no_session =
        || CommandError::new(ErrorKind::Refused, format!("no live Session in Box {name}"));
    let reload = || {
        store
            .box_row(id)?
            .ok_or_else(|| CommandError::new(ErrorKind::Failed, format!("no task {name}")))
    };
    let _held = BoxLock::acquire(
        cx.clock(),
        cx.state_dir()?,
        id,
        "end-session",
        boxlock::WAIT,
    )?;
    if asleep(&reload()?) {
        wake_and_revive(cx, store, &reload()?)?;
    }
    let row = reload()?;
    let (session, ordinal) = live_session(store, &row)?.ok_or_else(no_session)?;
    if let Some(launch) = store.live_session(id)? {
        let saved = transcript::save(cx, store, &row, &launch, Trigger::End);
        stop_agent(cx, store, &row, saved.as_ref().map(|t| t.digest.as_str()))
            .map_err(|e| reword(e, name))?;
    }
    if !store.end_current_session(id, "requested", None)? {
        return Err(no_session());
    }
    Ok(EndSessionValue {
        box_name: name.to_owned(),
        session,
        ordinal,
        box_kept: true,
    })
}

/// Row id and ordinal of the Box's current Session when it has not ended.
fn live_session(store: &Store, row: &BoxRow) -> Result<Option<(i64, i64)>, CommandError> {
    Ok(store
        .box_sessions(row.id)?
        .pop()
        .filter(|s| s.state != SessionState::Ended)
        .map(|s| (s.id, s.ordinal)))
}

/// A pane-close failure worded for `end-session`.
fn reword(e: CommandError, name: &str) -> CommandError {
    if e.kind != ErrorKind::Failed {
        return e;
    }
    let text = e
        .error
        .to_string()
        .replace("the task stays open", "the Session stays live");
    CommandError::new(e.kind, text).with_hint(format!(
        "try `moor end {name}` again, or close the pane by hand"
    ))
}

/// Whether the Task or its Box is asleep.
fn asleep(row: &BoxRow) -> bool {
    row.lifecycle == BoxLifecycle::Asleep
        || row
            .boxed
            .as_ref()
            .is_some_and(|b| b.state == BoxState::Asleep)
}

#[cfg(test)]
mod tests;
