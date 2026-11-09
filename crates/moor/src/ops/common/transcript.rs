//! Saves an archive of the agent's project directory to the blob store, after each turn and
//! before its session ends, and points the Box's Session at it.

use std::fs::{self, DirBuilder, Permissions};
use std::io::BufReader;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use moor_core::decide::transcript::{digest, is_duplicate, Trigger};
use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::domain::hands::HandsState;
use moor_core::seam::clock::Ts;
use moor_core::seam::driver::DriverError;
use moor_core::seam::engine::{EngineError, SessionRef};
use moor_core::seam::proc::Deadline;
use moor_core::{BoxId, Warning};

use crate::blobs::BlobKey;
use crate::cx::owners::Cmd;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::ops::common::activity::session_ref;
use crate::store::archive::ArchivePointer;
use crate::store::{BoxRow, SessionRow, Store};

/// A transcript saved in the blob store.
pub(crate) struct Saved {
    /// The archive's blob key.
    pub(crate) key: BlobKey,
    /// Where a person finds the archive.
    pub(crate) path: String,
    /// The digest of the archive.
    pub(crate) digest: String,
}

/// How long a failed save waits before the next settle pass tries it again.
const RETRY_AFTER: Duration = Duration::from_mins(1);

/// How a save ended.
enum Outcome {
    /// The archive is in the blob store and the Session points at it.
    Saved(Saved),
    /// There was nothing to save.
    Skipped,
    /// The save did not happen and may work later.
    Failed,
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
    match archive(cx, store, row, session, trigger) {
        Outcome::Saved(saved) => Some(saved),
        Outcome::Skipped | Outcome::Failed => None,
    }
}

/// [`save`], telling a save with nothing to save from one that may work later.
fn archive(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    session: &SessionRow,
    trigger: Trigger,
) -> Outcome {
    let Some(hands) = row.hands.as_ref().filter(|h| h.state == HandsState::Ready) else {
        return Outcome::Failed;
    };
    let known = |id: &Option<String>| id.as_deref().is_some_and(|id| !id.is_empty());
    if !known(&session.registered_agent_session) && !known(&row.agent_session_id) {
        record(store, row, skipped(trigger, "no agent session registered"));
        return Outcome::Skipped;
    }
    let Some(box_session) = session.session else {
        record(store, row, skipped(trigger, "no Session"));
        return Outcome::Skipped;
    };
    let Some(s) = session_ref(row, session) else {
        return Outcome::Failed;
    };
    let taken = cx.clock().now();
    let deadline = archive_deadline(trigger);
    let (event, outcome) = match try_save(cx, row, box_session, session, &s, deadline) {
        Ok((saved, fresh)) => {
            point(cx, store, row.id, box_session, &saved, (hands.inc, taken));
            if !fresh {
                return Outcome::Saved(saved);
            }
            let event = ExtraEvent::TranscriptSaved {
                trigger: trigger.name().to_owned(),
                path: saved.path.clone(),
                digest: saved.digest.clone(),
            };
            (event, Outcome::Saved(saved))
        }
        Err(TryError::NothingToArchive) => {
            (skipped(trigger, "nothing to archive"), Outcome::Skipped)
        }
        Err(TryError::Failed(error)) => (failed(trigger, error, false), Outcome::Failed),
        Err(TryError::Unlinked(error)) => (failed(trigger, error, true), Outcome::Failed),
    };
    record(store, row, event);
    outcome
}

/// Points Session `session` at `saved`, taken at `at` by hands `inc`; a store failure is a
/// warning, and the archive due stays for a later pass.
fn point(cx: &Cx, store: &Store, id: BoxId, session: i64, saved: &Saved, (inc, at): (i64, Ts)) {
    let to = ArchivePointer {
        key: saved.key.as_str().to_owned(),
        inc,
        at,
    };
    if let Err(e) = store.advance_archive(id, session, &to) {
        cx.warn(Warning {
            code: "archive_pointer_failed".to_owned(),
            message: format!("Box {}: {e}", id.get()),
        });
    }
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
/// holds the Box's turn, when the Box is no longer Ready, when the live session is another one,
/// or when its exit save already ran, failed or was skipped: none is retried.
pub(crate) fn save_on_exit(
    cx: &Cx,
    store: &Store,
    row: &BoxRow,
    probed: i64,
) -> Result<(), CommandError> {
    let Some(_turn) = cx.try_turn(row.id, Cmd::Settle) else {
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

/// Saves Box `id`'s archive when one is due, so losing the Box later keeps everything up to the
/// last turn.
///
/// A turn that ended made the archive due. While another operation holds the Box's turn nothing
/// is saved and the next settle pass tries again; a failed save is recorded and tried again a
/// minute later. The due is paid when the Session's pointer moves, or when there is nothing to
/// save.
pub(crate) fn save_due(cx: &Cx, store: &Store, id: BoxId) -> Result<(), CommandError> {
    let now = cx.clock().now();
    if store.archive_due(id)?.is_none_or(|due| due > now) {
        return Ok(());
    }
    let Some(_turn) = cx.try_turn(id, Cmd::Settle) else {
        return Ok(());
    };
    let Some(row) = store.box_row(id)? else {
        return Ok(());
    };
    let Some(session) = store.live_session(id)? else {
        store.reschedule_archive(id, now, None)?;
        return Ok(());
    };
    match archive(cx, store, &row, &session, Trigger::Turn) {
        Outcome::Saved(saved) => store.set_transcript_digest(session.id, &saved.digest)?,
        Outcome::Skipped => store.reschedule_archive(id, now, None)?,
        Outcome::Failed => {
            store.reschedule_archive(id, now, Some(now.saturating_add(RETRY_AFTER)))?;
        }
    }
    Ok(())
}

/// How long the Box may take to build and stream the archive for `trigger`.
///
/// A turn save holds the Box's turn that a steer or close waits on, and the next turn saves again,
/// so it gives up early. Every other save is the last chance to keep the session's transcript, and
/// a busy Box can take longer than a turn save allows.
const fn archive_deadline(trigger: Trigger) -> Deadline {
    match trigger {
        Trigger::Turn => Deadline(Duration::from_secs(15)),
        Trigger::Exit | Trigger::End | Trigger::Close | Trigger::Relaunch => {
            Deadline(Duration::from_mins(1))
        }
    }
}

/// Records `event`; a store failure is dropped so the caller carries on.
fn record(store: &Store, row: &BoxRow, event: ExtraEvent) {
    let _ = store.apply(Decision::new(row.id).event(event));
}

/// Archives the transcript to a staged file, reads back its digest and stores it as a blob
/// named by that digest under Session `box_session`.
///
/// The flag tells a new archive from a repeat of the one already stored, which is dropped.
fn try_save(
    cx: &Cx,
    row: &BoxRow,
    box_session: i64,
    session: &SessionRow,
    s: &SessionRef,
    deadline: Deadline,
) -> Result<(Saved, bool), TryError> {
    let staged = staged(cx, row, session)?;
    let result = archive_to(cx, s, &staged, deadline)
        .and_then(|()| store_archive(cx, row.id, box_session, session, &staged));
    let _ = fs::remove_file(&staged);
    result
}

/// Brings the archive to `staged`.
fn archive_to(cx: &Cx, s: &SessionRef, staged: &Path, deadline: Deadline) -> Result<(), TryError> {
    cx.engine()
        .transcript(s, staged, deadline)
        .map_err(|e| match e {
            EngineError::NothingToArchive => TryError::NothingToArchive,
            EngineError::Box(DriverError::Transport { .. }) => TryError::Unlinked(e.to_string()),
            e => TryError::Failed(e.to_string()),
        })
}

/// [`try_save`] once the archive is staged.
fn store_archive(
    cx: &Cx,
    id: BoxId,
    box_session: i64,
    session: &SessionRow,
    staged: &Path,
) -> Result<(Saved, bool), TryError> {
    fs::set_permissions(staged, Permissions::from_mode(0o600))
        .map_err(|e| format!("chmod {}: {e}", staged.display()))?;
    let file = fs::File::open(staged).map_err(|e| format!("read {}: {e}", staged.display()))?;
    let digest =
        digest(BufReader::new(file)).map_err(|e| format!("read {}: {e}", staged.display()))?;
    let key = BlobKey::transcript(id, box_session, &digest);
    let blobs = cx.blobs().map_err(|e| e.error.to_string())?;
    let fresh = !is_duplicate(session.transcript_digest.as_deref(), &digest);
    if fresh {
        blobs.put_file(&key, staged).map_err(|e| e.to_string())?;
    }
    let path = blobs.locate(&key);
    Ok((Saved { key, path, digest }, fresh))
}

/// A local path to stage launch `session`'s archive at, with its directory made (0700) and any
/// file a crashed save left there removed.
fn staged(cx: &Cx, row: &BoxRow, session: &SessionRow) -> Result<PathBuf, String> {
    let state = cx.state_dir().map_err(|e| e.error.to_string())?;
    let dir = state.join("transcripts").join(".staging");
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)
        .map_err(|e| format!("create {}: {e}", dir.display()))?;
    let path = dir.join(format!("{}-{}.tgz", row.id.get(), session.id));
    let _ = fs::remove_file(&path);
    Ok(path)
}

#[cfg(test)]
mod tests;
