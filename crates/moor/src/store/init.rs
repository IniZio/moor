//! The store's once-made `meta` values, its controller lock, and stored-value parsing helpers.

use std::fs::{self, File, TryLockError};
use std::path::Path;

use moor_core::box_key::ControllerId;
use moor_core::cache::CacheSalt;
use moor_schema::tables::Meta;
use sea_query::{Expr, ExprTrait as _, OnConflict, Query};

use super::{Store, StoreError};
use crate::env::CONTROLLER_LOCK;
use crate::store::db::{Db, DbExt as _};

impl Store {
    /// This state dir's cache key salt, made on first use and never changed.
    pub(crate) fn cache_salt(&self) -> Result<CacheSalt, StoreError> {
        let mut fresh = [0u8; 64];
        getrandom::fill(&mut fresh)
            .map_err(|e| StoreError::Corrupt(format!("no randomness for the cache salt: {e}")))?;
        let stored: String = {
            let conn = self.writer();
            meta_once(&*conn, "cache_salt", CacheSalt::new(fresh).to_hex())?
        };
        parsed_opt("meta cache_salt", &stored, CacheSalt::from_hex)
    }
}

impl Store {
    /// This store's controller id, made on first use and never changed: Box ids restart with a
    /// fresh store, so the id lives and dies with the database that numbers them.
    pub(crate) fn controller_id(&self) -> Result<ControllerId, StoreError> {
        let mut fresh = [0u8; 4];
        getrandom::fill(&mut fresh).map_err(|e| {
            StoreError::Corrupt(format!("no randomness for the controller id: {e}"))
        })?;
        let stored: String = {
            let conn = self.writer();
            meta_once(
                &*conn,
                "controller_id",
                ControllerId::from_bytes(fresh).as_str(),
            )?
        };
        parsed_opt("meta controller_id", &stored, ControllerId::parse)
    }
}

/// The `meta` value at `k`, set to `fresh` when it has none.
fn meta_once(
    conn: &dyn Db,
    k: &str,
    fresh: impl Into<sea_query::Value>,
) -> Result<String, StoreError> {
    conn.run(
        Query::insert()
            .into_table(Meta::Table)
            .columns([Meta::K, Meta::V])
            .values_panic([k.into(), Expr::val(fresh)])
            .on_conflict(OnConflict::column(Meta::K).do_nothing().to_owned()),
    )?;
    Ok(conn.row(
        Query::select()
            .column(Meta::V)
            .from(Meta::Table)
            .and_where(Expr::col(Meta::K).eq(k)),
        |r| r.get(0),
    )?)
}

/// The controller lock beside `db`, held until dropped; none when this process is the
/// controller holding it. Another holder is a refusal naming its pid and socket.
pub(super) fn owner_lock(db: &Path) -> Result<Option<File>, StoreError> {
    let path = db.with_file_name(CONTROLLER_LOCK);
    let file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .read(true)
        .open(&path)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(TryLockError::WouldBlock) => {
            let held = fs::read_to_string(&path).unwrap_or_default();
            let mut lines = held.lines();
            let pid = lines.next().unwrap_or("?").to_owned();
            if pid == std::process::id().to_string() {
                return Ok(None);
            }
            Err(StoreError::Owned {
                db: db.to_path_buf(),
                pid,
                socket: lines.next().unwrap_or("?").to_owned(),
            })
        }
        Err(TryLockError::Error(e)) => Err(e.into()),
    }
}

/// Parse a stored text value, naming the column on failure.
pub(super) fn parsed<T, E: std::fmt::Display>(
    what: &str,
    raw: &str,
    f: impl FnOnce(&str) -> Result<T, E>,
) -> Result<T, StoreError> {
    f(raw).map_err(|e| StoreError::Corrupt(format!("{what} `{raw}`: {e}")))
}

/// Parse a stored enum value that has an `Option`-returning parser.
pub(super) fn parsed_opt<T>(
    what: &str,
    raw: &str,
    f: impl FnOnce(&str) -> Option<T>,
) -> Result<T, StoreError> {
    parsed(what, raw, |s| f(s).ok_or("unknown value"))
}

#[cfg(test)]
mod tests;
