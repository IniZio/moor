//! Each repo's config, kept in the store as the YAML text `parse_repo` reads.
//!
//! A row is keyed by the repo's lowercased `owner/name`, since GitHub treats `IniZio/x` and
//! `inizio/x` as one repo. Every write is one statement that decides on the row's revision, so
//! two writers cannot both win.

use moor_core::name::ForgeRepo;
use moor_core::seam::clock::Ts;
use moor_schema::tables::Repos;
use sea_query::{Expr, ExprTrait as _, OnConflict, Query};

use crate::store::db::{DbExt as _, OptionalExtension};
use crate::store::{Store, StoreError};

/// A repo's stored config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RepoConfigRow {
    /// The YAML text, which parsed when it was stored.
    pub(crate) text: String,
    /// How many times the row was written; 1 for the first.
    pub(crate) revision: u64,
    /// Who wrote it last: `operator`, `import`, or the user's id as `user <n>`.
    pub(crate) edited_by: String,
    /// When it was written last.
    pub(crate) edited_at: Ts,
}

/// What a write of a repo's config did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Wrote {
    /// The row is now this.
    Written(RepoConfigRow),
    /// The revision the writer expected is not the row's; `current` is the row's, `None` when
    /// there is no row.
    Stale {
        /// The row's revision.
        current: Option<u64>,
    },
}

/// The key of `repo`'s row.
fn key(repo: &ForgeRepo) -> String {
    repo.to_string().to_ascii_lowercase()
}

impl Store {
    /// `repo`'s stored config, if it has one.
    pub(crate) fn repo_config(
        &self,
        repo: &ForgeRepo,
    ) -> Result<Option<RepoConfigRow>, StoreError> {
        let row = self
            .reader()?
            .row(
                Query::select()
                    .columns([
                        Repos::Config,
                        Repos::Revision,
                        Repos::EditedBy,
                        Repos::EditedAt,
                    ])
                    .from(Repos::Table)
                    .and_where(Expr::col(Repos::Repo).eq(key(repo))),
                |r| {
                    Ok(RepoConfigRow {
                        text: r.get(0)?,
                        revision: u64::try_from(r.get::<_, i64>(1)?).unwrap_or(0),
                        edited_by: r.get(2)?,
                        edited_at: Ts::from_millis(r.get(3)?),
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    /// Stores `text` as `repo`'s config when it has none, at revision 1; the row that is already
    /// there is left alone and `None` comes back.
    pub(crate) fn insert_repo_config(
        &self,
        repo: &ForgeRepo,
        text: &str,
        edited_by: &str,
    ) -> Result<Option<RepoConfigRow>, StoreError> {
        let made = self.writer().run(
            Query::insert()
                .into_table(Repos::Table)
                .columns([
                    Repos::Repo,
                    Repos::Config,
                    Repos::Revision,
                    Repos::EditedBy,
                    Repos::EditedAt,
                ])
                .values_panic([
                    key(repo).into(),
                    text.into(),
                    1_i64.into(),
                    edited_by.into(),
                    self.now().as_millis().into(),
                ])
                .on_conflict(OnConflict::column(Repos::Repo).do_nothing().to_owned()),
        )?;
        if made == 0 {
            return Ok(None);
        }
        self.repo_config(repo)
    }

    /// Stores `text` as `repo`'s config. With `expected`, the write happens only when the row is
    /// at that revision, where 0 means no row; without it the row is replaced whatever it holds.
    pub(crate) fn set_repo_config(
        &self,
        repo: &ForgeRepo,
        text: &str,
        edited_by: &str,
        expected: Option<u64>,
    ) -> Result<Wrote, StoreError> {
        let changed = match expected {
            Some(0) => {
                return Ok(match self.insert_repo_config(repo, text, edited_by)? {
                    Some(row) => Wrote::Written(row),
                    None => self.stale(repo)?,
                })
            }
            Some(revision) => self.writer().run(
                Query::update()
                    .table(Repos::Table)
                    .value(Repos::Config, text)
                    .value(Repos::Revision, Expr::col(Repos::Revision).add(1))
                    .value(Repos::EditedBy, edited_by)
                    .value(Repos::EditedAt, self.now().as_millis())
                    .and_where(Expr::col(Repos::Repo).eq(key(repo)))
                    .and_where(
                        Expr::col(Repos::Revision).eq(i64::try_from(revision).unwrap_or(i64::MAX)),
                    ),
            )?,
            None => self.writer().run(
                Query::insert()
                    .into_table(Repos::Table)
                    .columns([
                        Repos::Repo,
                        Repos::Config,
                        Repos::Revision,
                        Repos::EditedBy,
                        Repos::EditedAt,
                    ])
                    .values_panic([
                        key(repo).into(),
                        text.into(),
                        1_i64.into(),
                        edited_by.into(),
                        self.now().as_millis().into(),
                    ])
                    .on_conflict(
                        OnConflict::column(Repos::Repo)
                            .value(Repos::Config, text)
                            .value(
                                Repos::Revision,
                                Expr::col((Repos::Table, Repos::Revision)).add(1),
                            )
                            .value(Repos::EditedBy, edited_by)
                            .value(Repos::EditedAt, self.now().as_millis())
                            .to_owned(),
                    ),
            )?,
        };
        if changed == 0 {
            return self.stale(repo);
        }
        Ok(self
            .repo_config(repo)?
            .map_or_else(|| Wrote::Stale { current: None }, Wrote::Written))
    }

    /// The refusal of a write whose expected revision was not the row's.
    fn stale(&self, repo: &ForgeRepo) -> Result<Wrote, StoreError> {
        Ok(Wrote::Stale {
            current: self.repo_config(repo)?.map(|r| r.revision),
        })
    }
}

#[cfg(test)]
mod tests;
