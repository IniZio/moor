//! Puts the agent's conversation back on new hands before its session resumes.

use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::domain::event::RestoreEvent;
use moor_core::seam::clock::Ts;
use moor_core::seam::engine::{LaunchPrompt, LaunchReq};
use moor_core::BoxId;

use crate::blobs::BlobKey;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::view::rfc3339;
use crate::store::Store;

/// What an agent that starts without its earlier conversation reads first.
pub(crate) const LOST_LINE: &str = "This Box lost its machine and your earlier conversation \
     could not be brought back, so you start without it. A prompt you were working on when it \
     was lost was not sent again.";

/// Makes the Box that `launch` resumes on hold the session it resumes, from the Box's newest
/// archive when it lacks it; when the archive was taken, so the launch knows which sent steers
/// the restored conversation holds. When that fails, `launch` starts a fresh agent told the
/// conversation was lost. Either outcome is recorded as an event; a Box that holds the session
/// is left as it is.
///
/// # Errors
/// A store failure.
pub(super) fn before_resume(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    launch: &mut LaunchReq,
) -> Result<Option<Ts>, CommandError> {
    let Some(session) = launch.resume.clone() else {
        return Ok(None);
    };
    let (event, restored) = match bring_back(cx, store, id, launch, &session)? {
        Ok(None) => return Ok(None),
        Ok(Some(at)) => (
            RestoreEvent::SessionRestored {
                archive_at: rfc3339(at),
            },
            Some(at),
        ),
        Err(reason) => {
            start_without(launch);
            (RestoreEvent::SessionLost { reason }, None)
        }
    };
    store.apply(Decision::new(id).event(ExtraEvent::Restore(event)))?;
    Ok(restored)
}

/// When the restored archive was taken; none when the Box already held `session`. The inner
/// error says why it could not be restored.
fn bring_back(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    launch: &LaunchReq,
    session: &str,
) -> Result<Result<Option<Ts>, String>, CommandError> {
    match cx.engine().holds_session(launch, session) {
        Ok(true) => return Ok(Ok(None)),
        Ok(false) => {}
        Err(e) => return Ok(Err(e.to_string())),
    }
    let Some(newest) = store.newest_archive(id)? else {
        return Ok(Err("no archive was saved".to_owned()));
    };
    let archive = cx.blobs().map_err(|e| e.error.to_string()).and_then(|b| {
        b.get(&BlobKey::recorded(newest.key))
            .map_err(|e| e.to_string())
    });
    let restored = archive.and_then(|bytes| {
        cx.engine()
            .restore_session(launch, session, &bytes)
            .map_err(|e| e.to_string())
    });
    Ok(restored.map(|()| Some(newest.at)))
}

/// Makes `launch` start a fresh agent whose first prompt opens with [`LOST_LINE`].
fn start_without(launch: &mut LaunchReq) {
    launch.resume = None;
    let prompt = launch
        .prompt
        .text()
        .map_or_else(|| LOST_LINE.to_owned(), |p| format!("{LOST_LINE}\n\n{p}"));
    launch.prompt = LaunchPrompt::Text(prompt);
}
