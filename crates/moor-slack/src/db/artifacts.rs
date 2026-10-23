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
    /// An upload failed and the thread shows why; the version is still owed.
    Failure {
        /// Unix seconds of the first failure.
        since: i64,
        /// Unix seconds of the latest attempt.
        tried_at: i64,
        /// Attempts so far.
        tries: u32,
        /// Whether the thread shows the reason.
        told: bool,
    },
    /// The version itself, or its name and size when the file cannot go up.
    Version,
    /// The front stopped trying and the thread says so. Never tried again.
    GaveUp,
}

impl Db {
    /// What the thread of `box_name` shows of `version` of artifact `name`.
    pub(crate) fn artifact_shown(
        &self,
        box_name: &BoxName,
        name: &str,
        version: u32,
    ) -> rusqlite::Result<ArtifactShown> {
        let row = self
            .conn
            .query_row(
                "SELECT posted_at IS NOT NULL, gave_up_at IS NOT NULL, failed_at, tried_at, tries, told \
                 FROM slack_artifacts WHERE box_name = ?1 AND name = ?2 AND version = ?3",
                params![box_name.as_str(), name, version],
                |r| {
                    Ok((
                        r.get::<_, bool>(0)?,
                        r.get::<_, bool>(1)?,
                        r.get::<_, Option<i64>>(2)?,
                        r.get::<_, Option<i64>>(3)?,
                        r.get::<_, u32>(4)?,
                        r.get::<_, bool>(5)?,
                    ))
                },
            )
            .optional()?;
        Ok(match row {
            None => ArtifactShown::Nothing,
            Some((true, ..)) => ArtifactShown::Version,
            Some((_, true, ..)) => ArtifactShown::GaveUp,
            Some((_, _, since, tried_at, tries, told)) => ArtifactShown::Failure {
                since: since.unwrap_or_default(),
                tried_at: tried_at.unwrap_or_default(),
                tries,
                told,
            },
        })
    }

    /// Records that an upload of `version` of `name` failed at `now`, and whether the thread
    /// now shows the reason.
    pub(crate) fn artifact_failed(
        &self,
        box_name: &BoxName,
        name: &str,
        version: u32,
        now: i64,
        told: bool,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO slack_artifacts (box_name, name, version, failed_at, tried_at, tries, told) \
             VALUES (?1, ?2, ?3, ?4, ?4, 1, ?5) \
             ON CONFLICT (box_name, name, version) \
             DO UPDATE SET tried_at = excluded.tried_at, tries = tries + 1, \
                           told = MAX(told, excluded.told)",
            params![box_name.as_str(), name, version, now, told],
        )?;
        Ok(())
    }

    /// Records at `now` that the front gave up on `version` of `name` for good.
    pub(crate) fn artifact_gave_up(
        &self,
        box_name: &BoxName,
        name: &str,
        version: u32,
        now: i64,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO slack_artifacts (box_name, name, version, failed_at, tried_at, tries, gave_up_at) \
             VALUES (?1, ?2, ?3, ?4, ?4, 1, ?4) \
             ON CONFLICT (box_name, name, version) DO UPDATE SET gave_up_at = excluded.gave_up_at",
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
