//! Each artifact version a Box publishes, in the Box's thread: the file once per version, or
//! one name and size line when the bot cannot upload files. `slack.db` keys each version by
//! its Box, name and version, so a refold or a restart shows none twice. A version whose
//! upload fails shows the reason once and is tried again with a growing wait, for ten minutes
//! from its first failure. Then the thread says the front gave up and the version is never tried
//! again, not even after a restart.

use moor_core::name::{BoxId, BoxName};
use moor_core::view::ArtifactView;
use serde::Deserialize;
use serde_json::Value;

use super::prompts::box_id;
use super::{Feed, Fold};
use crate::db::{ArtifactShown, Db, SlackBox};
use crate::upload::{post_version, Attempt, Sent, Thread};

/// The payload of an `artifact_published` event.
#[derive(Deserialize)]
struct Published {
    /// Artifact name.
    name: String,
    /// Version published.
    version: u32,
    /// Artifact kind.
    artifact_kind: String,
    /// MIME content type.
    content_type: String,
    /// Size in bytes.
    size: u64,
    /// SHA-256 of the content, hex.
    sha256: String,
}

/// Applies one event of Box `id`: each `artifact_published` owes the thread that version.
pub(super) fn fold(fold: &mut Fold, id: BoxId, kind: &str, payload: &Value) {
    if kind != "artifact_published" {
        return;
    }
    let Ok(p) = Published::deserialize(payload) else {
        return;
    };
    fold.published.entry(id).or_default().push(ArtifactView {
        name: p.name,
        version: p.version,
        kind: p.artifact_kind,
        content_type: p.content_type,
        size: p.size,
        sha256: p.sha256,
        created_at: String::new(),
    });
}

impl Feed {
    /// Shows each version owed to the thread of `boxed`. Returns false only when `slack.db`
    /// could not be read or written. A version waiting out its retry delay, or whose upload
    /// failed, is recorded with its delay and holds back nothing else.
    pub(super) fn artifacts(&self, db: &Db, fold: &mut Fold, boxed: &SlackBox) -> bool {
        let Some(id) = box_id(&boxed.state) else {
            return true;
        };
        let Some(owed) = fold.published.get_mut(&id) else {
            return true;
        };
        let thread = Thread {
            api: &self.api,
            bot: &self.bot_token,
            channel: &boxed.channel,
            ts: &boxed.thread_ts,
            log: &*self.log,
        };
        let mut stuck = false;
        owed.retain(|v| match self.show(db, &thread, &boxed.box_name, v) {
            Shown::Settled => false,
            Shown::Owed => true,
            Shown::Stuck => {
                stuck = true;
                true
            }
        });
        !stuck
    }

    /// Shows `v` in `thread` unless it already does, and records what it shows. Uploading
    /// first means a crash before the record uploads the version again instead of losing it.
    fn show(&self, db: &Db, thread: &Thread<'_>, box_name: &BoxName, v: &ArtifactView) -> Shown {
        let now = (self.clock)();
        let attempt = match db.artifact_shown(box_name, &v.name, v.version) {
            Ok(ArtifactShown::Version | ArtifactShown::GaveUp) => return Shown::Settled,
            Ok(ArtifactShown::Nothing) => Attempt::First,
            Ok(ArtifactShown::Failure {
                since,
                tried_at,
                tries,
                told,
            }) => {
                if now < tried_at.saturating_add(backoff(tries)) {
                    return Shown::Owed;
                }
                if now.saturating_sub(since) >= GIVE_UP_AFTER {
                    Attempt::Last
                } else if told {
                    Attempt::Retry
                } else {
                    Attempt::First
                }
            }
            Err(e) => {
                (self.log)(&format!("slack.db: {e}"));
                return Shown::Stuck;
            }
        };
        let (settled, recorded) = match post_version(thread, &self.client, box_name, v, attempt) {
            Ok(Sent::Shown) => (true, db.artifact_posted(box_name, &v.name, v.version, now)),
            Ok(Sent::Failed(why)) => {
                (self.log)(&format!("slack: {} not uploaded: {why}", v.name));
                Self::record_failure(db, box_name, v, attempt, now, true)
            }
            Err(e) => {
                (self.log)(&format!("slack: artifact line failed: {e}"));
                Self::record_failure(db, box_name, v, attempt, now, false)
            }
        };
        match recorded {
            Ok(()) if settled => Shown::Settled,
            Ok(()) => Shown::Owed,
            Err(e) => {
                (self.log)(&format!("slack.db: {e}"));
                Shown::Stuck
            }
        }
    }

    /// Records a failed try at `now`: the last try gives up for good, any other schedules the
    /// next one. `told` says whether the thread now shows a line about it. Returns whether the
    /// version is settled, with the record's result.
    fn record_failure(
        db: &Db,
        box_name: &BoxName,
        v: &ArtifactView,
        attempt: Attempt,
        now: i64,
        told: bool,
    ) -> (bool, rusqlite::Result<()>) {
        if attempt == Attempt::Last {
            (true, db.artifact_gave_up(box_name, &v.name, v.version, now))
        } else {
            (
                false,
                db.artifact_failed(box_name, &v.name, v.version, now, told),
            )
        }
    }
}

/// What one pass over a version left.
enum Shown {
    /// Shown, or given up on.
    Settled,
    /// Still owed, with its next try recorded.
    Owed,
    /// `slack.db` failed, so nothing is recorded.
    Stuck,
}

/// Seconds after its first failure at which the front stops trying to upload a version.
const GIVE_UP_AFTER: i64 = 600;

/// Seconds to wait after the `tries`th failed attempt: 2, 4, 8, ... up to 64.
fn backoff(tries: u32) -> i64 {
    1_i64 << tries.clamp(1, 6)
}

#[cfg(test)]
mod tests;
