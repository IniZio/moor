//! The one-time import of the repo configs Boxes carried before configs lived in the store.
//!
//! A Box row kept the repo config text it started with, and the Boxes of a controller that ran
//! before the `repos` table hold the only copy of a repo's config. This copies the text of each
//! repo's newest Box into `repos`, as `import` at revision 1. It is Rust and not a numbered
//! migration, because a migration is SQL and cannot tell which text still parses.

use std::collections::HashMap;

use moor_core::config::parse_repo;
use moor_core::name::ForgeRepo;
use moor_schema::tables::{Boxes, Meta};
use sea_query::{Expr, ExprTrait as _, Func, OnConflict, Query};
use serde::Deserialize;

use crate::store::db::{DbExt as _, OptionalExtension};
use crate::store::{Store, StoreError};

/// What a row imported from a Box records as the one who wrote it.
const IMPORT: &str = "import";

/// The `meta` key that records the import ran.
const DONE: &str = "repo_config_imported";

/// What the import did.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Imported {
    /// The repos that now hold an imported config.
    pub(crate) stored: Vec<ForgeRepo>,
    /// The repos whose newest text was left out, each with why.
    pub(crate) skipped: Vec<(ForgeRepo, String)>,
}

/// One entry of a Box's `other_repos` column, as an older controller wrote it.
#[derive(Deserialize)]
struct Listed {
    /// `OWNER/NAME`.
    repo: ForgeRepo,
    /// The repo's config text at the base.
    #[serde(rename = "moor_yaml")]
    text: Option<String>,
}

/// A repo's newest Box that names it, with the text that Box holds.
struct Newest {
    /// The Box's id.
    id: i64,
    /// The repo as that Box spells it.
    repo: ForgeRepo,
    /// The text, none when the Box held none.
    text: Option<String>,
}

impl Store {
    /// Stores, for each repo with no config, the text of its newest Box, unless it does not
    /// parse; then records that it ran, so no later start imports again. A repo whose newest Box
    /// held no text gets none. A store failure leaves the record unwritten, so the next start
    /// tries again.
    ///
    /// # Errors
    /// A store failure.
    pub(crate) fn import_repo_configs(&self) -> Result<Imported, StoreError> {
        let mut done = Imported::default();
        if self.import_ran()? {
            return Ok(done);
        }
        let mut newest = self.newest_per_repo()?.into_values().collect::<Vec<_>>();
        newest.sort_by_key(|n| n.id);
        for Newest { repo, text, .. } in newest {
            let Some(text) = text.filter(|t| !t.trim().is_empty()) else {
                continue;
            };
            if self.repo_config(&repo)?.is_some() {
                continue;
            }
            if let Err(e) = parse_repo(&format!("{repo}:newest Box's config"), &text) {
                done.skipped.push((repo, e.to_string()));
            } else if self.insert_repo_config(&repo, &text, IMPORT)?.is_some() {
                done.stored.push(repo);
            }
        }
        self.writer().run(
            Query::insert()
                .into_table(Meta::Table)
                .columns([Meta::K, Meta::V])
                .values_panic([DONE.into(), self.now().as_millis().to_string().into()])
                .on_conflict(OnConflict::column(Meta::K).do_nothing().to_owned()),
        )?;
        Ok(done)
    }

    /// Whether the import has run on this store.
    fn import_ran(&self) -> Result<bool, StoreError> {
        let mark: Option<String> = self
            .reader()?
            .row(
                Query::select()
                    .column(Meta::V)
                    .from(Meta::Table)
                    .and_where(Expr::col(Meta::K).eq(DONE)),
                |r| r.get(0),
            )
            .optional()?;
        Ok(mark.is_some())
    }

    /// Each repo, by its lowercased `OWNER/NAME`, with the newest Box that names it as its repo
    /// or as one listed beside it.
    fn newest_per_repo(&self) -> Result<HashMap<String, Newest>, StoreError> {
        let latest = Query::select()
            .expr(Expr::col(Boxes::Id).max())
            .from(Boxes::Table)
            .and_where(Expr::col(Boxes::Repo).is_not_null())
            .add_group_by([Func::lower(Expr::col(Boxes::Repo)).into()])
            .take();
        let primary = self.reader()?.rows(
            Query::select()
                .columns([Boxes::Id, Boxes::Repo, Boxes::RepoConfig])
                .from(Boxes::Table)
                .and_where(Expr::col(Boxes::Id).in_subquery(latest)),
            |r| {
                Ok::<_, StoreError>((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                ))
            },
        )?;
        let mut all = HashMap::new();
        for row in primary {
            let (id, repo, text) = row?;
            // A stored repo that is not `OWNER/NAME` has no config to give.
            if let Ok(repo) = ForgeRepo::parse(&repo) {
                keep(&mut all, Newest { id, repo, text });
            }
        }
        let others = self.reader()?.rows(
            Query::select()
                .columns([Boxes::Id, Boxes::OtherRepos])
                .from(Boxes::Table)
                .and_where(Expr::col(Boxes::OtherRepos).is_not_null()),
            |r| Ok::<_, StoreError>((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
        )?;
        for row in others {
            let (id, raw) = row?;
            // A column that does not read lists nothing, as it did when a Box started.
            for Listed { repo, text } in
                serde_json::from_str::<Vec<Listed>>(&raw).unwrap_or_default()
            {
                keep(&mut all, Newest { id, repo, text });
            }
        }
        Ok(all)
    }
}

/// Keeps `found` unless `all` holds a repo's entry from a newer Box, or from the same Box.
fn keep(all: &mut HashMap<String, Newest>, found: Newest) {
    let key = found.repo.to_string().to_ascii_lowercase();
    if all.get(&key).is_none_or(|held| held.id < found.id) {
        all.insert(key, found);
    }
}

#[cfg(test)]
mod tests;
