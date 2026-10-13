//! Where a saved transcript goes and how it is told apart: pure naming and digesting.

use std::fmt::Write as _;

use sha2::{Digest, Sha256};

use crate::name::TaskId;

/// What ended the session whose transcript is saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// The agent was started again.
    Relaunch,
    /// The Task was closed.
    Close,
}

impl Trigger {
    /// The name recorded in events.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Relaunch => "relaunch",
            Self::Close => "close",
        }
    }
}

/// The number for the next save: one more than the kinds that say a transcript was saved.
#[must_use]
pub fn next_number<'a>(kinds: impl IntoIterator<Item = &'a str>) -> usize {
    kinds
        .into_iter()
        .filter(|k| *k == "transcript_saved")
        .count()
        + 1
}

/// The archive path under the state directory: `transcripts/<task>/<session>-<n>`.
#[must_use]
pub fn relative_path(task: TaskId, session: i64, n: usize) -> String {
    format!("transcripts/{}/{session}-{n}", task.get())
}

/// Lowercase hex sha256 of the archive bytes.
#[must_use]
pub fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut acc, b| {
            let _ = write!(acc, "{b:02x}");
            acc
        })
}

#[cfg(test)]
mod tests;
