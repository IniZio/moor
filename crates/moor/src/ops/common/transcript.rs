//! Saves the agent's transcript to the blob store before its session ends.

use std::fs::{self, DirBuilder, Permissions};
use std::io::BufReader;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use moor_core::decide::transcript::{digest, is_duplicate, next_number, Trigger};
use moor_core::domain::boxes::BoxState;
use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::seam::driver::DriverError;
use moor_core::seam::engine::{EngineError, SessionRef};

use crate::blobs::BlobKey;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::activity::session_ref;
use crate::ops::common::boxlock::BoxLock;
use crate::store::{BoxRow, EventRow, SessionRow, Store};

/// A transcript saved in the blob store.
pub(crate) struct Saved {
    /// Where a person finds the archive.
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
    row: &BoxRow,
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
    let s = session_ref(row, session)?;
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
        Err(TryError::Failed(error)) => failed(trigger, error, false),
        Err(TryError::Unlinked(error)) => failed(trigger, error, true),
    };
    record(store, row, event);
    None
}

/// Records that no save was made for `trigger`, with the reason.
pub(crate) fn record_skipped(store: &Store, row: &BoxRow, trigger: Trigger, reason: &str) {
    record(store, row, skipped(trigger, reason));
}

/// The event for a save that failed; `unlinked` marks a Box whose machine was out of reach.
fn failed(trigger: Trigger, error: String, unlinked: bool) -> ExtraEvent {
    ExtraEvent::TranscriptFailed {
        trigger: trigger.name().to_owned(),
        error,
        unlinked,
    }
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
    /// The Box's machine was out of reach, so a later save may work.
    Unlinked(String),
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
/// or when its exit save already ran, failed or was skipped: none is retried.
pub(crate) fn save_on_exit(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
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
    let Some(row) = store.box_row(row.id)? else {
        return Ok(());
    };
    let Some(session) = store.live_session(row.id)?.filter(|s| s.id == probed) else {
        return Ok(());
    };
    if exit_save_ended(store, &row)? {
        return Ok(());
    }
    if let Some(saved) = save(cx, store, &row, &session, Trigger::Exit) {
        store.set_transcript_digest(session.id, &saved.digest)?;
    }
    Ok(())
}

/// Whether an exit save ran, failed or was skipped since the Box's latest session started.
fn exit_save_ended(store: &Store, row: &BoxRow) -> Result<bool, CommandError> {
    let events = store.events(row.id, usize::MAX)?;
    Ok(events
        .iter()
        .rev()
        .take_while(|e| e.kind != "session_started")
        .any(|e| {
            [
                "transcript_saved",
                "transcript_failed",
                "transcript_skipped",
            ]
            .contains(&e.kind.as_str())
                && e.payload.get("trigger").and_then(|t| t.as_str()) == Some("exit")
        }))
}

/// Copies the live session's transcript to the host at the end of a turn, so losing the Box
/// later keeps everything up to that turn. Nothing is copied while another operation holds the
/// Box lock; a failed copy is recorded and the next turn tries again.
pub(crate) fn save_on_turn(cx: &Cx, store: &Store, row: &BoxRow) -> Result<(), CommandError> {
    let Ok(_held) = BoxLock::acquire(
        cx.clock(),
        cx.state_dir()?,
        row.id,
        "transcript",
        Duration::ZERO,
    ) else {
        return Ok(());
    };
    let Some(row) = store.box_row(row.id)? else {
        return Ok(());
    };
    let Some(session) = store.live_session(row.id)? else {
        return Ok(());
    };
    if let Some(saved) = save(cx, store, &row, &session, Trigger::Turn) {
        store.set_transcript_digest(session.id, &saved.digest)?;
    }
    Ok(())
}

/// Whether the live session's latest save failed because the Box's machine was not linked.
fn pending(store: &Store, row: &BoxRow) -> Result<bool, CommandError> {
    let events = store.events(row.id, usize::MAX)?;
    let latest = events
        .iter()
        .rev()
        .take_while(|e| e.kind != "session_started")
        .find(|e| e.kind.starts_with("transcript_"));
    Ok(latest.is_some_and(|e| {
        e.kind == "transcript_failed"
            && e.payload
                .get("unlinked")
                .and_then(serde_json::Value::as_bool)
                == Some(true)
    }))
}

/// Saves again the transcript whose last save failed for want of a link, now that the link is
/// back. The archive holds every turn so far, so one save covers each turn that failed. A failure
/// for the same reason stays pending; a save equal to the stored one ends the pending state with
/// a skip, since nothing was lost.
pub(crate) fn save_pending(cx: &Cx, store: &Store, row: &BoxRow) -> Result<(), CommandError> {
    if !pending(store, row)? {
        return Ok(());
    }
    let Ok(_held) = BoxLock::acquire(
        cx.clock(),
        cx.state_dir()?,
        row.id,
        "transcript",
        Duration::ZERO,
    ) else {
        return Ok(());
    };
    let Some(row) = store.box_row(row.id)? else {
        return Ok(());
    };
    let Some(session) = store.live_session(row.id)? else {
        return Ok(());
    };
    if let Some(saved) = save(cx, store, &row, &session, Trigger::Turn) {
        store.set_transcript_digest(session.id, &saved.digest)?;
        if pending(store, &row)? {
            record_skipped(store, &row, Trigger::Turn, "already saved");
        }
    }
    Ok(())
}

/// Records `event`; a store failure is dropped so the caller carries on.
fn record(store: &Store, row: &BoxRow, event: ExtraEvent) {
    let _ = store.apply(Decision::new(row.id).event(event));
}

/// Archives the transcript to a staged file, reads back its digest and stores it as a blob.
///
/// None when the archive repeats the stored digest but the earlier save is not on record.
/// The flag tells a new archive from a repeat of the one already stored, which is dropped.
fn try_save(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    session: &SessionRow,
    s: &SessionRef,
) -> Result<Option<(Saved, bool)>, TryError> {
    let events = store
        .events(row.id, usize::MAX)
        .map_err(|e| e.to_string())?;
    let n = next_number(events.iter().map(|e| e.kind.as_str()));
    let key = BlobKey::transcript(row.id, session.id, n);
    let staged = staged(cx, row, session, n)?;
    let result = archive_and_store(cx, s, session, &events, &key, &staged);
    let _ = fs::remove_file(&staged);
    result
}

/// [`try_save`] once it knows where the archive is staged.
fn archive_and_store(
    cx: &Cx,
    s: &SessionRef,
    session: &SessionRow,
    events: &[EventRow],
    key: &BlobKey,
    staged: &Path,
) -> Result<Option<(Saved, bool)>, TryError> {
    cx.engine().transcript(s, staged).map_err(|e| match e {
        EngineError::NothingToArchive => TryError::NothingToArchive,
        EngineError::Box(DriverError::Transport { .. }) => TryError::Unlinked(e.to_string()),
        e => TryError::Failed(e.to_string()),
    })?;
    fs::set_permissions(staged, Permissions::from_mode(0o600))
        .map_err(|e| format!("chmod {}: {e}", staged.display()))?;
    let file = fs::File::open(staged).map_err(|e| format!("read {}: {e}", staged.display()))?;
    let digest =
        digest(BufReader::new(file)).map_err(|e| format!("read {}: {e}", staged.display()))?;
    if is_duplicate(session.transcript_digest.as_deref(), &digest) {
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
    let blobs = cx.blobs().map_err(|e| e.error.to_string())?;
    blobs.put_file(key, staged).map_err(|e| e.to_string())?;
    let path = blobs.locate(key);
    Ok(Some((Saved { path, digest }, true)))
}

/// A local path to archive save `n` to, with its directory made (0700).
fn staged(cx: &Cx, row: &BoxRow, session: &SessionRow, n: usize) -> Result<PathBuf, String> {
    let state = cx.state_dir().map_err(|e| e.error.to_string())?;
    let dir = state.join("transcripts").join(".staging");
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)
        .map_err(|e| format!("create {}: {e}", dir.display()))?;
    Ok(dir.join(format!("{}-{}-{n}.tgz", row.id.get(), session.id)))
}

#[cfg(test)]
mod tests;
