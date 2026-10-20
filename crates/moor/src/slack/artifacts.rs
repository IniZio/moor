//! Artifact versions in Slack threads: the file itself, or one text line when it cannot go up.

use moor_core::secret::Secret;
use moor_core::BoxId;

use super::api::{Api, ApiError, Upload};
use super::status::escape;
use crate::store::{SlackThread, Store};

/// Slack's error code when the bot token lacks the `files:write` scope.
const MISSING_SCOPE: &str = "missing_scope";

/// One published artifact version.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Published {
    /// Box that published it.
    box_id: BoxId,
    /// File name.
    name: String,
    /// Version number, starting at 1.
    version: i64,
    /// Size in bytes.
    size: u64,
}

/// Why a version did not go up.
enum NotUploaded {
    /// The bot token lacks `files:write`.
    MissingScope,
    /// Anything else, as a plain reason.
    Failed(String),
}

/// Shows `v` in `thread`: uploads the stored file, or posts one text line when that fails.
/// Errors when nothing reached the thread, so the caller can try again.
fn show(
    store: &Store,
    api: &Api,
    bot: &Secret,
    thread: &SlackThread,
    v: &Published,
) -> Result<(), ApiError> {
    let name = escape(&v.name);
    let text = match upload(store, api, bot, thread, v) {
        Ok(()) => return Ok(()),
        Err(NotUploaded::MissingScope) => format!("{name} ({})", size_text(v.size)),
        Err(NotUploaded::Failed(why)) => {
            format!("{name} could not be uploaded: {}", escape(&why))
        }
    };
    api.post_message(bot, &thread.channel, &thread.thread_ts, &text)
        .map(drop)
}

/// Streams the stored file of `v` into `thread`.
fn upload(
    store: &Store,
    api: &Api,
    bot: &Secret,
    thread: &SlackThread,
    v: &Published,
) -> Result<(), NotUploaded> {
    let mut file = match store.open_artifact(v.box_id, &v.name, v.version) {
        Ok(Some(file)) => file,
        Ok(None) => return Err(NotUploaded::Failed("no stored copy".to_owned())),
        Err(e) => {
            return Err(NotUploaded::Failed(format!(
                "cannot read the stored copy ({e})"
            )))
        }
    };
    let mut file = Upload {
        name: &v.name,
        len: v.size,
        body: &mut file,
    };
    api.upload_file(bot, &thread.channel, &thread.thread_ts, &mut file)
        .map_err(|e| match e {
            ApiError::Slack(code) if code == MISSING_SCOPE => NotUploaded::MissingScope,
            e => NotUploaded::Failed(e.to_string()),
        })
}

/// A size as moor-web shows it: bytes below 1 KB, else KB, MB or GB, with one decimal below 10.
fn size_text(n: u64) -> String {
    if n < 1024 {
        return format!("{n} B");
    }
    let n = u128::from(n);
    let (mut unit, mut base) = ("KB", 1024_u128);
    for next in ["MB", "GB"] {
        if n < base * 1024 {
            break;
        }
        (unit, base) = (next, base * 1024);
    }
    if n < base * 10 {
        let tenths = (n * 10 + base / 2) / base;
        format!("{}.{} {unit}", tenths / 10, tenths % 10)
    } else {
        format!("{} {unit}", (n + base / 2) / base)
    }
}

#[cfg(test)]
mod tests;
