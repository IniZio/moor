//! Each user's agent profile repo. The repo is read later with the user's own `github` secret,
//! so these rows hold no credential.

use moor_core::name::{ForgeRepo, ProfileSource};
use moor_core::user::UserId;
use moor_schema::tables::UserProfiles;
use sea_query::{Expr, ExprTrait as _, OnConflict, Query};

use crate::store::db::{DbExt as _, OptionalExtension};
use crate::store::{Store, StoreError};

impl Store {
    /// Records `source` as `user`'s profile, replacing any earlier one.
    pub(crate) fn set_user_profile(
        &self,
        user: UserId,
        source: &ProfileSource,
    ) -> Result<(), StoreError> {
        self.writer().run(
            Query::insert()
                .into_table(UserProfiles::Table)
                .columns([
                    UserProfiles::User,
                    UserProfiles::Repo,
                    UserProfiles::GitRef,
                    UserProfiles::UpdatedAt,
                ])
                .values_panic([
                    user.get().into(),
                    source.repo.to_string().into(),
                    source.git_ref.as_deref().into(),
                    self.now().as_millis().into(),
                ])
                .on_conflict(
                    OnConflict::column(UserProfiles::User)
                        .update_columns([
                            UserProfiles::Repo,
                            UserProfiles::GitRef,
                            UserProfiles::UpdatedAt,
                        ])
                        .to_owned(),
                ),
        )?;
        Ok(())
    }

    /// Clears `user`'s profile; true when there was one.
    pub(crate) fn delete_user_profile(&self, user: UserId) -> Result<bool, StoreError> {
        let gone = self.writer().run(
            Query::delete()
                .from_table(UserProfiles::Table)
                .and_where(Expr::col(UserProfiles::User).eq(user.get())),
        )?;
        Ok(gone == 1)
    }

    /// `user`'s profile, if one is set.
    pub(crate) fn user_profile(&self, user: UserId) -> Result<Option<ProfileSource>, StoreError> {
        let row: Option<(String, Option<String>)> = self
            .reader()?
            .row(
                Query::select()
                    .columns([UserProfiles::Repo, UserProfiles::GitRef])
                    .from(UserProfiles::Table)
                    .and_where(Expr::col(UserProfiles::User).eq(user.get())),
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        row.map(|(repo, git_ref)| {
            let repo = ForgeRepo::parse(&repo)
                .map_err(|e| StoreError::Corrupt(format!("user_profiles repo {repo}: {e}")))?;
            Ok(ProfileSource { repo, git_ref })
        })
        .transpose()
    }
}
