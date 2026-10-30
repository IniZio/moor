//! Each user's agent profile repo. The repo is read later with the user's own `github` secret,
//! so these rows hold no credential.

use moor_core::name::{ForgeRepo, ProfileSource};
use moor_core::user::UserId;
use rusqlite::{params, OptionalExtension};

use crate::store::{Store, StoreError};

impl Store {
    /// Records `source` as `user`'s profile, replacing any earlier one.
    pub(crate) fn set_user_profile(
        &self,
        user: UserId,
        source: &ProfileSource,
    ) -> Result<(), StoreError> {
        self.writer().execute(
            "INSERT INTO user_profiles (user, repo, git_ref, updated_at) VALUES (?, ?, ?, ?) \
             ON CONFLICT (user) DO UPDATE SET repo = excluded.repo, git_ref = excluded.git_ref, \
             updated_at = excluded.updated_at",
            params![
                user.get(),
                source.repo.to_string(),
                source.git_ref,
                self.now().as_millis()
            ],
        )?;
        Ok(())
    }

    /// Clears `user`'s profile; true when there was one.
    pub(crate) fn delete_user_profile(&self, user: UserId) -> Result<bool, StoreError> {
        let gone = self
            .writer()
            .execute("DELETE FROM user_profiles WHERE user = ?", [user.get()])?;
        Ok(gone == 1)
    }

    /// `user`'s profile, if one is set.
    pub(crate) fn user_profile(&self, user: UserId) -> Result<Option<ProfileSource>, StoreError> {
        let row: Option<(String, Option<String>)> = self
            .reader()?
            .query_row(
                "SELECT repo, git_ref FROM user_profiles WHERE user = ?",
                [user.get()],
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
