//! The repo columns of a Box row.

use moor_core::domain::decision::BoxRepo;
use moor_core::seam::forge::{ForgeRepo, GitAuthor};
use rusqlite::Row;

use super::init::parsed;
use super::StoreError;

/// The Box's repo columns; a stored repo that is not `OWNER/NAME` is an invalid row.
pub(super) fn repo_from_row(row: &Row<'_>) -> Result<Option<BoxRepo>, StoreError> {
    let Some(repo) = row.get::<_, Option<String>>(2)? else {
        return Ok(None);
    };
    Ok(Some(BoxRepo {
        repo: parsed("box repo", &repo, ForgeRepo::parse)?,
        moor_yaml: row.get(30)?,
        author: GitAuthor {
            name: row.get::<_, Option<String>>(31)?.unwrap_or_default(),
            email: row.get::<_, Option<String>>(32)?.unwrap_or_default(),
        },
    }))
}
