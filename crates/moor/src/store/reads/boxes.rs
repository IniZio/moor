//! Lookups of open Boxes by repo and by checkout path.

use super::super::{BoxRow, Store, StoreError};
use super::query_boxes;

impl Store {
    /// Open Boxes of `repo`, oldest id first.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the database fails.
    #[cfg_attr(not(test), expect(dead_code, reason = "no caller outside tests yet"))]
    pub(crate) fn open_boxes_by_repo(&self, repo: &str) -> Result<Vec<BoxRow>, StoreError> {
        let tail = "WHERE b.repo = ?1 AND b.state NOT IN ('closed', 'rejected') ORDER BY b.id";
        query_boxes(&self.reader()?, tail, [repo])
    }

    /// The open Box whose checkout is `path`, newest first when several match.
    ///
    /// A Box matches on its stored worktree, bound or not: a bound Box stores
    /// the user's checkout path there.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the database fails.
    #[cfg_attr(not(test), expect(dead_code, reason = "no caller outside tests yet"))]
    pub(crate) fn open_box_by_path(&self, path: &str) -> Result<Option<BoxRow>, StoreError> {
        let tail = "WHERE b.state NOT IN ('closed', 'rejected') AND b.worktree = ?1 \
            ORDER BY b.id DESC LIMIT 1";
        let rows = query_boxes(&self.reader()?, tail, [path])?;
        Ok(rows.into_iter().next())
    }
}

#[cfg(test)]
mod tests;
