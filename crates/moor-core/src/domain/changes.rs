//! What a turn changed in git, as a receipt records it: files, and lines added and removed.

use serde::{Deserialize, Serialize};

/// The most files a receipt lists; the rest are only counted.
pub const RECEIPT_FILES: usize = 200;

/// One file's change: its lines added and removed, none for a binary file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChange {
    /// The path now.
    pub path: String,
    /// The path before, when git found the file renamed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// Lines added; none for a binary file.
    pub added: Option<u64>,
    /// Lines removed; none for a binary file.
    pub removed: Option<u64>,
}

/// Files changed and lines added and removed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tally {
    /// Files changed.
    pub files: u64,
    /// Lines added.
    pub added: u64,
    /// Lines removed.
    pub removed: u64,
}

impl Tally {
    /// The sum over `files`.
    #[must_use]
    pub fn of(files: &[FileChange]) -> Self {
        files.iter().fold(Self::default(), |t, f| Self {
            files: t.files + 1,
            added: t.added + f.added.unwrap_or(0),
            removed: t.removed + f.removed.unwrap_or(0),
        })
    }

    /// This and `other` together.
    #[must_use]
    pub const fn plus(self, other: Self) -> Self {
        Self {
            files: self.files + other.files,
            added: self.added + other.added,
            removed: self.removed + other.removed,
        }
    }
}

/// One repo's part of a turn receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoReceipt {
    /// The repo, as `OWNER/NAME`.
    pub repo: String,
    /// What the turn changed, against the work tree the last receipt saw.
    pub turn: Tally,
    /// Everything against the base.
    pub total: Tally,
    /// The first [`RECEIPT_FILES`] files of `total`.
    pub files: Vec<FileChange>,
}

/// One repo of what `changes` answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoChanges {
    /// The repo, as `OWNER/NAME`.
    pub repo: String,
    /// Everything against the base.
    pub total: Tally,
    /// Each file changed; a receipt keeps only the first [`RECEIPT_FILES`].
    pub files: Vec<FileChange>,
    /// The unified diff against the base, cut at the byte budget; none for a receipt that kept no diff.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch: Option<String>,
    /// True when the patch was cut.
    #[serde(default)]
    pub cut: bool,
    /// Why the repo could not be read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
}
