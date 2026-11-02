//! The agent profile a Box resolved at its start, kept so a relaunch reuses it unread.

use moor_core::name::ForgeRepo;
use moor_core::{BoxId, Sha};
use rusqlite::{params, OptionalExtension};

use crate::store::{Store, StoreError};

/// A profile resolved for one Box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BoxProfile {
    /// The profile repo.
    pub(crate) repo: ForgeRepo,
    /// The commit the Box checks out.
    pub(crate) sha: Sha,
    /// `moor-profile.yaml` at `sha`; `None` when absent or unreadable.
    pub(crate) manifest: Option<String>,
}

impl Store {
    /// Records `profile` for Box `id`, replacing any earlier one.
    pub(crate) fn set_box_profile(
        &self,
        id: BoxId,
        profile: &BoxProfile,
    ) -> Result<(), StoreError> {
        self.writer().execute(
            "INSERT INTO box_profiles (box, repo, sha, manifest) VALUES (?, ?, ?, ?) \
             ON CONFLICT (box) DO UPDATE SET repo = excluded.repo, sha = excluded.sha, \
             manifest = excluded.manifest",
            params![
                id.get(),
                profile.repo.to_string(),
                profile.sha.as_str(),
                profile.manifest
            ],
        )?;
        Ok(())
    }

    /// The profile Box `id` resolved, if any.
    pub(crate) fn box_profile(&self, id: BoxId) -> Result<Option<BoxProfile>, StoreError> {
        let row: Option<(String, String, Option<String>)> = self
            .reader()?
            .query_row(
                "SELECT repo, sha, manifest FROM box_profiles WHERE box = ?",
                [id.get()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let corrupt = |what: &str, v: &str| StoreError::Corrupt(format!("box_profiles {what} {v}"));
        row.map(|(repo, sha, manifest)| {
            Ok(BoxProfile {
                repo: ForgeRepo::parse(&repo).map_err(|_| corrupt("repo", &repo))?,
                sha: Sha::parse(&sha).map_err(|_| corrupt("sha", &sha))?,
                manifest,
            })
        })
        .transpose()
    }
}
