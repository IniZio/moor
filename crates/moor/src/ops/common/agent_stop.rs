//! Stopping a task's live agent: close its pane, end its session.

use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::seam::surface::{PaneRef, SurfaceError};
use moor_core::ErrorKind;

use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::{BoxRow, Store};

/// Closes the live agent pane and ends its session, with the digest of its saved transcript;
/// a pane already gone is fine.
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
    if let Some(pane) = session.pane.as_deref() {
        match cx.surface().close_pane(&PaneRef::new(pane)) {
            Ok(()) | Err(SurfaceError::NotFound) => {}
            Err(e) => {
                return Err(CommandError::new(
                    ErrorKind::Failed,
                    format!("could not close the agent pane: {e}; the task stays open"),
                )
                .with_hint("try `moor close` again, or close the pane by hand"));
            }
        }
    }
    store.end_session(session.id, None, digest)?;
    store.apply(Decision::new(row.id).event(ExtraEvent::SessionsStopped {
        sessions: vec![session.session_ref],
    }))?;
    Ok(true)
}
