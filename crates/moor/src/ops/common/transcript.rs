//! Saves the agent's transcript to the state directory before its session ends.

use std::fs::{self, DirBuilder, Permissions};
use std::io::BufReader;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::PathBuf;
use std::time::Duration;

use moor_core::decide::transcript::{digest, is_duplicate, next_number, relative_path, Trigger};
use moor_core::domain::boxes::BoxState;
use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::seam::engine::{EngineError, SessionRef};

use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::activity::session_ref;
use crate::ops::common::boxlock::BoxLock;
use crate::store::{SessionRow, Store, TaskRow};

/// A transcript saved on the host.
pub(crate) struct Saved {
    /// Where the archive is.
    pub(crate) path: String,
    /// The digest of the archive.
    pub(crate) digest: String,
}

/// Saves the transcript of `session` while its Box is Ready, and records the outcome.
///
/// Nothing is saved when the Box is not Ready or cannot be reached. A failure is recorded as
/// `transcript_failed` and never stops the caller. An archive equal to the session's stored
/// digest writes no file and records no event; the earlier save is returned.
pub(crate) fn save(
    cx: &Cx,
    store: &Store,
    row: &TaskRow,
    session: &SessionRow,
    trigger: Trigger,
) -> Option<Saved> {
    if row.boxed.as_ref()?.state != BoxState::Ready {
        return None;
    }
    let known = |id: &Option<String>| id.as_deref().is_some_and(|id| !id.is_empty());
    if !known(&session.registered_agent_session) && !known(&row.agent_session_id) {
        record(store, row, skipped(trigger, "no agent session registered"));
        return None;
    }
    let s = session_ref(row, session.session_ref.clone(), session.pane.clone())?;
    let event = match try_save(cx, store, row, session, &s) {
        Ok(None) => return None,
        Ok(Some((saved, false))) => return Some(saved),
        Ok(Some((saved, true))) => {
            let event = ExtraEvent::TranscriptSaved {
                trigger: trigger.name().to_owned(),
                path: saved.path.clone(),
                digest: saved.digest.clone(),
            };
            record(store, row, event);
            return Some(saved);
        }
        Err(TryError::NothingToArchive) => skipped(trigger, "nothing to archive"),
        Err(TryError::Failed(error)) => ExtraEvent::TranscriptFailed {
            trigger: trigger.name().to_owned(),
            error,
        },
    };
    record(store, row, event);
    None
}

/// The event for a save that found nothing to save.
fn skipped(trigger: Trigger, reason: &str) -> ExtraEvent {
    ExtraEvent::TranscriptSkipped {
        trigger: trigger.name().to_owned(),
        reason: reason.to_owned(),
    }
}

/// Why `try_save` saved nothing new.
enum TryError {
    /// The agent left no transcript.
    NothingToArchive,
    /// The archive could not be built, copied or read.
    Failed(String),
}

impl From<String> for TryError {
    fn from(error: String) -> Self {
        Self::Failed(error)
    }
}

/// Saves the live session's transcript the first time its agent is seen exited.
///
/// `probed` is the session the exit was read from. Nothing is saved while another operation
/// holds the Box lock, when the Box is no longer Ready, when the live session is another one,
/// or when it was already saved or its exit save already failed or was skipped: neither is retried.
/// The digest kept on the session marks it saved.
pub(crate) fn save_on_exit(
    cx: &Cx,
    store: &Store,
    row: &TaskRow,
    probed: i64,
) -> Result<(), CommandError> {
    let Ok(_held) = BoxLock::acquire(
        cx.clock(),
        cx.state_dir()?,
        row.id,
        "transcript",
        Duration::ZERO,
    ) else {
        return Ok(());
    };
    let Some(row) = store.task(row.id)? else {
        return Ok(());
    };
    let Some(session) = store.live_session(row.id)?.filter(|s| s.id == probed) else {
        return Ok(());
    };
    if session.transcript_digest.is_some() || exit_save_ended(store, &row)? {
        return Ok(());
    }
    if let Some(saved) = save(cx, store, &row, &session, Trigger::Exit) {
        store.set_transcript_digest(session.id, &saved.digest)?;
    }
    Ok(())
}

/// Whether an exit save failed or was skipped since the Task's latest session started.
fn exit_save_ended(store: &Store, row: &TaskRow) -> Result<bool, CommandError> {
    let events = store.events(row.id, usize::MAX)?;
    Ok(events
        .iter()
        .rev()
        .take_while(|e| e.kind != "session_started")
        .any(|e| {
            (e.kind == "transcript_failed" || e.kind == "transcript_skipped")
                && e.payload.get("trigger").and_then(|t| t.as_str()) == Some("exit")
        }))
}

/// Records `event`; a store failure is dropped so the caller carries on.
fn record(store: &Store, row: &TaskRow, event: ExtraEvent) {
    let _ = store.apply(Decision::new(row.id).event(event));
}

/// Archives the transcript to the state directory and reads back its digest.
///
/// None when the archive repeats the stored digest but the earlier save is not on record.
/// The flag tells a new archive from a repeat of the one already stored, which is removed.
fn try_save(
    cx: &Cx,
    store: &Store,
    row: &TaskRow,
    session: &SessionRow,
    s: &SessionRef,
) -> Result<Option<(Saved, bool)>, TryError> {
    let events = store
        .events(row.id, usize::MAX)
        .map_err(|e| e.to_string())?;
    let n = next_number(events.iter().map(|e| e.kind.as_str()));
    let dest = dest(cx, relative_path(row.id, session.id, n))?;
    cx.engine().transcript(s, &dest).map_err(|e| match e {
        EngineError::NothingToArchive => TryError::NothingToArchive,
        e => TryError::Failed(e.to_string()),
    })?;
    fs::set_permissions(&dest, Permissions::from_mode(0o600))
        .map_err(|e| format!("chmod {}: {e}", dest.display()))?;
    let file = fs::File::open(&dest).map_err(|e| format!("read {}: {e}", dest.display()))?;
    let digest =
        digest(BufReader::new(file)).map_err(|e| format!("read {}: {e}", dest.display()))?;
    if is_duplicate(session.transcript_digest.as_deref(), &digest) {
        let _ = fs::remove_file(&dest);
        let saved = events
            .iter()
            .rev()
            .filter(|e| e.kind == "transcript_saved")
            .find(|e| e.payload.get("digest").and_then(|d| d.as_str()) == Some(&digest))
            .and_then(|e| e.payload.get("path")?.as_str())
            .map(|path| {
                (
                    Saved {
                        path: path.to_owned(),
                        digest,
                    },
                    false,
                )
            });
        return Ok(saved);
    }
    let path = dest.display().to_string();
    Ok(Some((Saved { path, digest }, true)))
}

/// The host path for `rel` under the state directory, with its directory made (0700).
fn dest(cx: &Cx, rel: String) -> Result<PathBuf, String> {
    let path = cx.state_dir().map_err(|e| e.error.to_string())?.join(rel);
    if let Some(dir) = path.parent() {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(|e| format!("create {}: {e}", dir.display()))?;
    }
    Ok(path)
}

#[cfg(test)]
mod tests;
