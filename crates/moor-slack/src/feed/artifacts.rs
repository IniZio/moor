//! Each artifact version a Box publishes, in the Box's thread: the file once per version, or
//! one name and size line when the bot cannot upload files. `slack.db` keys each version by
//! its Box, name and version, so a refold or a restart shows none twice. A version whose
//! upload fails shows the reason once and is tried again on a later feed tick.

use moor_core::name::{BoxId, BoxName};
use moor_core::view::ArtifactView;
use serde::Deserialize;
use serde_json::Value;

use super::prompts::box_id;
use super::{Feed, Fold};
use crate::db::{ArtifactShown, Db, SlackBox};
use crate::upload::{post_version, Sent, Thread};

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
    /// Shows each version owed to the thread of `boxed`. Returns false when one is still owed.
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
        };
        owed.retain(|v| !self.show(db, &thread, &boxed.box_name, v));
        owed.is_empty()
    }

    /// Shows `v` in `thread` unless it already does, and records what it shows. Returns
    /// whether the thread shows the version. Uploading first means a crash before the record
    /// uploads the version again instead of losing it.
    fn show(&self, db: &Db, thread: &Thread<'_>, box_name: &BoxName, v: &ArtifactView) -> bool {
        let shown = match db.artifact_shown(box_name, &v.name, v.version) {
            Ok(ArtifactShown::Version) => return true,
            Ok(shown) => shown,
            Err(e) => {
                (self.log)(&format!("slack.db: {e}"));
                return false;
            }
        };
        let told = shown == ArtifactShown::Failure;
        let (done, recorded) = match post_version(thread, &self.client, box_name, v, told) {
            Ok(Sent::Shown) => (
                true,
                db.artifact_posted(box_name, &v.name, v.version, (self.clock)()),
            ),
            Ok(Sent::Failed(why)) => {
                (self.log)(&format!("slack: {} not uploaded: {why}", v.name));
                (
                    false,
                    db.artifact_failed(box_name, &v.name, v.version, (self.clock)()),
                )
            }
            Err(e) => {
                (self.log)(&format!("slack: artifact line failed: {e}"));
                return false;
            }
        };
        match recorded {
            Ok(()) => done,
            Err(e) => {
                (self.log)(&format!("slack.db: {e}"));
                false
            }
        }
    }
}

#[cfg(test)]
mod tests;
