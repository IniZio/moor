//! Each artifact version the front showed in a Box's thread, keyed by the Box, the artifact
//! name and the version, so a refold or a restart shows no version twice.

use moor_core::name::BoxName;
use rusqlite::{params, OptionalExtension};

use super::Db;

/// What the thread already shows of one artifact version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArtifactShown {
    /// Nothing yet.
    Nothing,
    /// Why its upload failed; the version is still owed.
    Failure,
    /// The version itself, or its name and size when the file cannot go up.
    Version,
}

impl Db {
    /// What the thread of `box_name` shows of `version` of artifact `name`.
    pub(crate) fn artifact_shown(
        &self,
        box_name: &BoxName,
        name: &str,
        version: u32,
    ) -> rusqlite::Result<ArtifactShown> {
        let posted: Option<bool> = self
            .conn
            .query_row(
                "SELECT posted_at IS NOT NULL FROM slack_artifacts \
                 WHERE box_name = ?1 AND name = ?2 AND version = ?3",
                params![box_name.as_str(), name, version],
                |r| r.get(0),
            )
            .optional()?;
        Ok(match posted {
            None => ArtifactShown::Nothing,
            Some(false) => ArtifactShown::Failure,
            Some(true) => ArtifactShown::Version,
        })
    }

    /// Records at `now` that the thread shows why `version` of `name` failed to go up.
    pub(crate) fn artifact_failed(
        &self,
        box_name: &BoxName,
        name: &str,
        version: u32,
        now: i64,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO slack_artifacts (box_name, name, version, failed_at) \
             VALUES (?1, ?2, ?3, ?4)",
            params![box_name.as_str(), name, version, now],
        )?;
        Ok(())
    }

    /// Records at `now` that the thread shows `version` of `name`.
    pub(crate) fn artifact_posted(
        &self,
        box_name: &BoxName,
        name: &str,
        version: u32,
        now: i64,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO slack_artifacts (box_name, name, version, posted_at) \
             VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT (box_name, name, version) DO UPDATE SET posted_at = excluded.posted_at",
            params![box_name.as_str(), name, version, now],
        )?;
        Ok(())
    }
}
