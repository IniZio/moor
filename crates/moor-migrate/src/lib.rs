//! Forward-only schema migrations for moor's `SQLite` databases, run by `refinery`.
//!
//! A database's schema is an ordered list of `V<n>__<name>.sql` files. [`migrate`] applies the
//! ones a file lacks and records each in `refinery_schema_history`. A file made before that
//! table existed is adopted at the baseline (migration 1) when its old version marker matches
//! the last pre-migration build, and refused otherwise.

use refinery_core::traits::sync::{Migrate, Query, Transaction};
use refinery_core::{Runner, Target};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

pub use refinery_core::Migration;

/// Where refinery records applied migrations.
const HISTORY: &str = "refinery_schema_history";

/// What a file without a migration history holds, judged by its old version marker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Legacy {
    /// A new, empty file: every migration runs.
    Empty,
    /// Written by the last build before migrations: adopted as already at migration 1.
    Baseline,
    /// Any other old version, named as the file records it.
    Other(String),
}

/// Why a database could not be brought to the latest migration.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A file from before migrations at a version this build cannot adopt.
    #[error("schema version `{0}` predates migrations and is not the adoptable baseline")]
    Unadoptable(String),
    /// The file has migrations this build does not know, so a newer build wrote it.
    #[error("the database is at migration {found} and this build knows only up to {known}")]
    Newer {
        /// Highest migration recorded in the file.
        found: i64,
        /// Highest migration this build carries.
        known: i64,
    },
    /// A migration file could not be named or an applied one was edited.
    #[error("migration: {0}")]
    Migration(#[from] refinery_core::Error),
    /// `SQLite` failed.
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

/// Build the migration list from `(file name, sql)` pairs in version order.
pub fn migrations(files: &[(&str, &str)]) -> Result<Vec<Migration>, Error> {
    Ok(files
        .iter()
        .map(|(name, sql)| Migration::unapplied(name.trim_end_matches(".sql"), sql))
        .collect::<Result<_, _>>()?)
}

/// The latest migration in `set`, 0 for an empty set.
#[must_use]
pub fn latest(set: &[Migration]) -> i64 {
    set.iter()
        .map(|m| i64::from(m.version()))
        .max()
        .unwrap_or(0)
}

/// Refuse, without writing, a file this build would not migrate.
pub fn check(
    conn: &Connection,
    set: &[Migration],
    legacy: impl FnOnce(&Connection) -> rusqlite::Result<Legacy>,
) -> Result<(), Error> {
    if has_history(conn)? {
        return refuse_newer(conn, set);
    }
    match legacy(conn)? {
        Legacy::Other(found) => Err(Error::Unadoptable(found)),
        Legacy::Empty | Legacy::Baseline => Ok(()),
    }
}

/// Bring the file to the latest migration in `set` inside one `BEGIN IMMEDIATE` transaction,
/// so a second process opening the file waits for the whole run and never sees half of it.
pub fn migrate(
    conn: &mut Connection,
    set: &[Migration],
    legacy: impl FnOnce(&Connection) -> rusqlite::Result<Legacy>,
) -> Result<(), Error> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let runner = || {
        Runner::new(set)
            .set_grouped(true)
            .set_abort_divergent(true)
            .set_abort_missing(true)
    };
    if has_history(&tx)? {
        refuse_newer(&tx, set)?;
    } else {
        match legacy(&tx)? {
            Legacy::Other(found) => return Err(Error::Unadoptable(found)),
            Legacy::Baseline => {
                runner()
                    .set_target(Target::FakeVersion(1))
                    .run(&mut Held(&tx))?;
            }
            Legacy::Empty => {}
        }
    }
    runner().run(&mut Held(&tx))?;
    tx.commit()?;
    Ok(())
}

/// True when the file records migrations.
fn has_history(conn: &Connection) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
        [HISTORY],
        |r| r.get(0),
    )
}

/// Refuse a file whose recorded migrations run past `set`.
fn refuse_newer(conn: &Connection, set: &[Migration]) -> Result<(), Error> {
    let found: Option<i64> = conn
        .query_row(&format!("SELECT MAX(version) FROM {HISTORY}"), [], |r| {
            r.get(0)
        })
        .optional()?
        .flatten();
    let known = latest(set);
    match found {
        Some(found) if found > known => Err(Error::Newer { found, known }),
        _ => Ok(()),
    }
}

/// refinery's view of a transaction this crate already holds. refinery's own rusqlite driver
/// opens a deferred transaction per step, which lets two processes read the history before
/// either writes it; running every step inside one immediate transaction serialises them.
struct Held<'a, 'c>(&'a rusqlite::Transaction<'c>);

impl Transaction for Held<'_, '_> {
    type Error = rusqlite::Error;

    fn execute<'q, T: Iterator<Item = &'q str>>(&mut self, queries: T) -> rusqlite::Result<usize> {
        let mut count = 0;
        for query in queries {
            self.0.execute_batch(query)?;
            count += 1;
        }
        Ok(count)
    }
}

impl Query<Vec<Migration>> for Held<'_, '_> {
    fn query(&mut self, query: &str) -> rusqlite::Result<Vec<Migration>> {
        let mut stmt = self.0.prepare(query)?;
        let rows = stmt.query_map([], |r| {
            let on: String = r.get(2)?;
            let checksum: String = r.get(3)?;
            Ok((r.get(0)?, r.get(1)?, on, checksum))
        })?;
        rows.map(|row| {
            let (version, name, on, checksum) = row?;
            let applied_on = OffsetDateTime::parse(&on, &Rfc3339).map_err(|e| corrupt(2, e))?;
            let checksum = checksum.parse::<u64>().map_err(|e| corrupt(3, e))?;
            Ok(Migration::applied(version, name, applied_on, checksum))
        })
        .collect()
    }
}

impl Migrate for Held<'_, '_> {}

/// A history column that does not parse.
fn corrupt(col: usize, e: impl std::error::Error + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(col, rusqlite::types::Type::Text, Box::new(e))
}

#[cfg(test)]
mod tests;
