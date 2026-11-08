//! Stopping a Box's live agent: end its terminal and its session.

use moor_core::domain::decision::{Decision, ExtraEvent};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::activity::session_ref;
use crate::store::{BoxRow, Store};

/// Ends the live agent's terminal and its session, with the digest of its saved transcript.
///
/// Returns whether a session was ended.
pub(crate) fn stop_agent(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    digest: Option<&str>,
) -> Result<bool, CommandError> {
    let Some(session) = store.live_session(row.id)? else {
        return Ok(false);
    };
    let sref = session_ref(row, &session);
    // Best effort: a Box that cannot be reached cannot run the agent on, and a close must still
    // go ahead.
    if let Some(s) = sref.filter(|_| !row.asleep()) {
        let _ = cx.engine().stop(&s);
    }
    store.end_session(session.id, None, digest)?;
    store.apply(Decision::new(row.id).event(ExtraEvent::SessionsStopped {
        sessions: vec![session.session_ref],
    }))?;
    Ok(true)
}
