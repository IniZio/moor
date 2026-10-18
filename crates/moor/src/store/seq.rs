//! Per-repo numbering for Boxes inserted without a user-chosen name.
//!
//! Each repo has a counter in `repo_seq`. It is bumped inside the same
//! transaction that inserts the Box, so a rolled-back insert hands out nothing,
//! and a number is never reused once a Box has taken it, even after the Box
//! closes.

use moor_core::domain::box_steps;
use moor_core::domain::decision::NewTask;
use moor_core::domain::task::TaskInput;
use moor_core::name::next_box_name;
use moor_core::{BoxId, BoxName};
use rusqlite::{Transaction, TransactionBehavior};

use super::{Store, StoreError};

/// Allocate the next free `<repo>/<n>` name inside `tx`.
///
/// Bumps the repo's counter until the name is not held by an open Box (a user
/// may have picked `<repo>/<n>` by hand). Names held only by closed or
/// rejected Boxes count as free, but their number is still never reissued
/// because the counter only grows.
///
/// # Errors
/// Returns [`StoreError::Invalid`] when the repo cannot form a valid name, and
/// [`StoreError::Sqlite`] when the database fails.
pub(super) fn next_name(tx: &Transaction<'_>, repo: &str) -> Result<BoxName, StoreError> {
    loop {
        let n: i64 = tx.query_row(
            "INSERT INTO repo_seq (repo, next) VALUES (?1, 1) \
             ON CONFLICT(repo) DO UPDATE SET next = next + 1 RETURNING next",
            [repo],
            |row| row.get(0),
        )?;
        let prev = u64::try_from(n - 1)
            .map_err(|_| StoreError::Invalid(format!("repo counter out of range: {n}")))?;
        let name = next_box_name(repo, &[prev]).map_err(|e| StoreError::Invalid(e.to_string()))?;
        let taken: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM boxes \
             WHERE name = ?1 AND state NOT IN ('closed', 'rejected'))",
            [name.as_str()],
            |row| row.get(0),
        )?;
        if !taken {
            return Ok(name);
        }
    }
}

impl Store {
    /// Insert a Box for `repo` with a store-assigned `<repo>/<n>` name.
    ///
    /// The name and the insert share one transaction, so the counter moves only
    /// when the Box exists.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the name is invalid or the database fails.
    #[cfg_attr(not(test), expect(dead_code, reason = "no caller outside tests yet"))]
    pub(crate) fn insert_unnamed(&self, repo: &str) -> Result<(BoxId, BoxName), StoreError> {
        let mut conn = self.writer()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let name = next_name(&tx, repo)?;
        let new = NewTask {
            name: name.clone(),
            repo: repo.to_owned(),
        };
        let (lifecycle, _) = box_steps::initial(&TaskInput::Insert)?;
        let id = super::apply::insert_task(&tx, &new, lifecycle, self.now())?;
        tx.commit()?;
        drop(conn);
        Ok((id, name))
    }
}

#[cfg(test)]
mod tests;
