//! Saves the agent's transcript to the state directory before its session ends.

use std::fs::{self, DirBuilder, Permissions};
use std::io::BufReader;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::PathBuf;

use moor_core::decide::transcript::{digest, next_number, relative_path, Trigger};
use moor_core::domain::boxes::BoxState;
use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::seam::engine::SessionRef;

use crate::cx::Cx;
use crate::ops::common::activity::session_ref;
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
/// `transcript_failed` and never stops the caller.
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
    let s = session_ref(row, session.session_ref.clone(), session.pane.clone())?;
    let event = match try_save(cx, store, row, session, &s) {
        Ok(saved) => {
            let event = ExtraEvent::TranscriptSaved {
                trigger: trigger.name().to_owned(),
                path: saved.path.clone(),
                digest: saved.digest.clone(),
            };
            record(store, row, event);
            return Some(saved);
        }
        Err(error) => ExtraEvent::TranscriptFailed {
            trigger: trigger.name().to_owned(),
            error,
        },
    };
    record(store, row, event);
    None
}

/// Records `event`; a store failure is dropped so the caller carries on.
fn record(store: &Store, row: &TaskRow, event: ExtraEvent) {
    let _ = store.apply(Decision::new(row.id).event(event));
}

/// Archives the transcript to the state directory and reads back its digest.
fn try_save(
    cx: &Cx,
    store: &Store,
    row: &TaskRow,
    session: &SessionRow,
    s: &SessionRef,
) -> Result<Saved, String> {
    let events = store
        .events(row.id, usize::MAX)
        .map_err(|e| e.to_string())?;
    let n = next_number(events.iter().map(|e| e.kind.as_str()));
    let dest = dest(cx, relative_path(row.id, session.id, n))?;
    cx.engine()
        .transcript(s, &dest)
        .map_err(|e| e.to_string())?;
    fs::set_permissions(&dest, Permissions::from_mode(0o600))
        .map_err(|e| format!("chmod {}: {e}", dest.display()))?;
    let file = fs::File::open(&dest).map_err(|e| format!("read {}: {e}", dest.display()))?;
    let digest =
        digest(BufReader::new(file)).map_err(|e| format!("read {}: {e}", dest.display()))?;
    Ok(Saved {
        path: dest.display().to_string(),
        digest,
    })
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
