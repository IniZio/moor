//! The repo columns of a Box row.

use moor_core::domain::decision::{BoxRepo, ListedRepo};
use moor_core::seam::forge::{ForgeRepo, GitAuthor};
#[cfg(test)]
use moor_core::BoxId;
use serde::Deserialize;

use super::init::parsed;
use super::StoreError;
use crate::store::db::Row;

/// One entry of the `other_repos` column, a JSON array in listed order.
#[derive(Deserialize)]
struct StoredRepo {
    /// `OWNER/NAME`.
    repo: ForgeRepo,
    /// The repo's `.moor.yaml` at the base.
    moor_yaml: Option<String>,
}

/// The Box's repo columns; a stored repo that is not `OWNER/NAME` is an invalid row.
pub(super) fn repo_from_row(row: &Row<'_>) -> Result<Option<BoxRepo>, StoreError> {
    let Some(repo) = row.get::<_, Option<String>>(2)? else {
        return Ok(None);
    };
    let others = match row.get::<_, Option<String>>(35)? {
        Some(raw) => parsed("box other_repos", &raw, |s| {
            serde_json::from_str::<Vec<StoredRepo>>(s)
        })?
        .into_iter()
        .map(|r| ListedRepo {
            repo: r.repo,
            moor_yaml: r.moor_yaml,
        })
        .collect(),
        None => Vec::new(),
    };
    Ok(Some(BoxRepo {
        repo: parsed("box repo", &repo, ForgeRepo::parse)?,
        moor_yaml: row.get(30)?,
        author: GitAuthor {
            name: row.get::<_, Option<String>>(31)?.unwrap_or_default(),
            email: row.get::<_, Option<String>>(32)?.unwrap_or_default(),
        },
        others,
    }))
}

/// The `other_repos` value of `repo`; `None` for a Box of one repo, so its row stays as before.
pub(super) fn others_column(repo: &BoxRepo) -> Option<String> {
    if repo.others.is_empty() {
        return None;
    }
    let stored: Vec<serde_json::Value> = repo
        .others
        .iter()
        .map(|r| serde_json::json!({ "repo": r.repo.to_string(), "moor_yaml": r.moor_yaml }))
        .collect();
    Some(serde_json::Value::from(stored).to_string())
}

#[cfg(test)]
impl super::Store {
    /// Lists `others` after Box `id`'s primary repo, as a start that chose them would.
    pub(crate) fn list_others(&self, id: BoxId, others: Vec<ListedRepo>) -> Result<(), StoreError> {
        use crate::store::db::DbExt as _;
        let Some(mut repo) = self.box_row(id)?.and_then(|row| row.repo) else {
            return Ok(());
        };
        repo.others = others;
        self.writer().execute(
            "UPDATE boxes SET other_repos = ?1 WHERE id = ?2",
            crate::store::db::params![others_column(&repo), id.get()],
        )?;
        Ok(())
    }
}
