//! The agent profile a Box resolved at its start, kept so a relaunch reuses it unread.

use moor_core::name::ForgeRepo;
use moor_core::{BoxId, Sha};
use moor_schema::tables::BoxProfiles;
use sea_query::{Expr, ExprTrait as _, OnConflict, Query};

use crate::store::db::{DbExt as _, OptionalExtension};
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
        self.writer().run(
            Query::insert()
                .into_table(BoxProfiles::Table)
                .columns([
                    BoxProfiles::Box,
                    BoxProfiles::Repo,
                    BoxProfiles::Sha,
                    BoxProfiles::Manifest,
                ])
                .values_panic([
                    id.get().into(),
                    profile.repo.to_string().into(),
                    profile.sha.as_str().into(),
                    profile.manifest.clone().into(),
                ])
                .on_conflict(
                    OnConflict::column(BoxProfiles::Box)
                        .update_columns([
                            BoxProfiles::Repo,
                            BoxProfiles::Sha,
                            BoxProfiles::Manifest,
                        ])
                        .to_owned(),
                ),
        )?;
        Ok(())
    }

    /// The profile Box `id` resolved, if any.
    pub(crate) fn box_profile(&self, id: BoxId) -> Result<Option<BoxProfile>, StoreError> {
        let row: Option<(String, String, Option<String>)> = self
            .reader()?
            .row(
                Query::select()
                    .columns([BoxProfiles::Repo, BoxProfiles::Sha, BoxProfiles::Manifest])
                    .from(BoxProfiles::Table)
                    .and_where(Expr::col(BoxProfiles::Box).eq(id.get())),
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
