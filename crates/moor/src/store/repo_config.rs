//! Each repo's texts in moor: its config, which `parse_repo` reads, in `repos`, and its Proxy
//! rules, which `parse_rules_text` reads, in `repo_rules`.
//!
//! A row is keyed by the repo's lowercased `owner/name`, since GitHub treats `IniZio/x` and
//! `inizio/x` as one repo. Every write is one statement that decides on the row's revision, so
//! two writers cannot both win.

use moor_core::name::ForgeRepo;
use moor_core::request::RepoFile;
use moor_core::seam::clock::Ts;
use moor_schema::tables::{RepoRules, Repos};
use sea_query::{DynIden, Expr, ExprTrait as _, IntoIden as _, OnConflict, Query};

use crate::store::db::{DbExt as _, OptionalExtension};
use crate::store::{Store, StoreError};

/// A repo's stored text.
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

/// What a write of a repo's text did.
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

/// The table and columns that hold one kind of repo text.
struct Cols {
    /// The table.
    table: DynIden,
    /// The repo key.
    repo: DynIden,
    /// The text.
    text: DynIden,
    /// The write count.
    revision: DynIden,
    /// Who wrote it last.
    edited_by: DynIden,
    /// When it was written last.
    edited_at: DynIden,
}

/// Where `file` is kept.
fn cols(file: RepoFile) -> Cols {
    match file {
        RepoFile::Config => Cols {
            table: Repos::Table.into_iden(),
            repo: Repos::Repo.into_iden(),
            text: Repos::Config.into_iden(),
            revision: Repos::Revision.into_iden(),
            edited_by: Repos::EditedBy.into_iden(),
            edited_at: Repos::EditedAt.into_iden(),
        },
        RepoFile::Rules => Cols {
            table: RepoRules::Table.into_iden(),
            repo: RepoRules::Repo.into_iden(),
            text: RepoRules::Rules.into_iden(),
            revision: RepoRules::Revision.into_iden(),
            edited_by: RepoRules::EditedBy.into_iden(),
            edited_at: RepoRules::EditedAt.into_iden(),
        },
    }
}

impl Store {
    /// The stored `file` of `repo`, if any.
    pub(crate) fn repo_text(
        &self,
        repo: &ForgeRepo,
        file: RepoFile,
    ) -> Result<Option<RepoConfigRow>, StoreError> {
        let c = cols(file);
        let row = self
            .reader()?
            .row(
                Query::select()
                    .columns([c.text, c.revision, c.edited_by, c.edited_at])
                    .from(c.table)
                    .and_where(Expr::col(c.repo).eq(key(repo))),
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

    /// Stores `text` as `repo`'s `file` at revision 1 unless it already has one; the new row,
    /// or `None` when a row was there.
    pub(crate) fn insert_repo_text(
        &self,
        repo: &ForgeRepo,
        file: RepoFile,
        text: &str,
        edited_by: &str,
    ) -> Result<Option<RepoConfigRow>, StoreError> {
        let c = cols(file);
        let made = self.writer().run(
            Query::insert()
                .into_table(c.table)
                .columns([c.repo.clone(), c.text, c.revision, c.edited_by, c.edited_at])
                .values_panic([
                    key(repo).into(),
                    text.into(),
                    1_i64.into(),
                    edited_by.into(),
                    self.now().as_millis().into(),
                ])
                .on_conflict(OnConflict::column(c.repo).do_nothing().to_owned()),
        )?;
        if made == 0 {
            return Ok(None);
        }
        self.repo_text(repo, file)
    }

    /// Replaces `repo`'s `file` with `text` when its revision is `expected` (0: none stored);
    /// with no `expected` it replaces whatever is stored.
    pub(crate) fn set_repo_text(
        &self,
        repo: &ForgeRepo,
        file: RepoFile,
        text: &str,
        edited_by: &str,
        expected: Option<u64>,
    ) -> Result<Wrote, StoreError> {
        let c = cols(file);
        let now = self.now().as_millis();
        let changed = match expected {
            Some(0) => {
                return Ok(match self.insert_repo_text(repo, file, text, edited_by)? {
                    Some(row) => Wrote::Written(row),
                    None => self.stale(repo, file)?,
                })
            }
            Some(revision) => self.writer().run(
                Query::update()
                    .table(c.table)
                    .value(c.text, text)
                    .value(c.revision.clone(), Expr::col(c.revision.clone()).add(1))
                    .value(c.edited_by, edited_by)
                    .value(c.edited_at, now)
                    .and_where(Expr::col(c.repo).eq(key(repo)))
                    .and_where(
                        Expr::col(c.revision).eq(i64::try_from(revision).unwrap_or(i64::MAX)),
                    ),
            )?,
            None => self.writer().run(
                Query::insert()
                    .into_table(c.table.clone())
                    .columns([
                        c.repo.clone(),
                        c.text.clone(),
                        c.revision.clone(),
                        c.edited_by.clone(),
                        c.edited_at.clone(),
                    ])
                    .values_panic([
                        key(repo).into(),
                        text.into(),
                        1_i64.into(),
                        edited_by.into(),
                        now.into(),
                    ])
                    .on_conflict(
                        OnConflict::column(c.repo)
                            .value(c.text, text)
                            .value(c.revision.clone(), Expr::col((c.table, c.revision)).add(1))
                            .value(c.edited_by, edited_by)
                            .value(c.edited_at, now)
                            .to_owned(),
                    ),
            )?,
        };
        if changed == 0 {
            return self.stale(repo, file);
        }
        Ok(self
            .repo_text(repo, file)?
            .map_or_else(|| Wrote::Stale { current: None }, Wrote::Written))
    }

    /// The refusal of a write that expected another revision of `repo`'s `file`.
    fn stale(&self, repo: &ForgeRepo, file: RepoFile) -> Result<Wrote, StoreError> {
        Ok(Wrote::Stale {
            current: self.repo_text(repo, file)?.map(|r| r.revision),
        })
    }
}

#[cfg(test)]
mod tests;
