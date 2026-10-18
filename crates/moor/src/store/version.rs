//! Retires a state database written by the old table layout.
//!
//! There is no migration: the old file is moved aside and a fresh store is
//! created in its place.

use std::fs::{self, File, TryLockError};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags, OptionalExtension};

use super::StoreError;

/// The `schema_version` value written by the old table layout.
const OLD_VERSION: &str = "1";

/// Moves a database with the old table layout to `<db>.v1.bak` (or the first
/// free numbered name) so a fresh store can be created at `db`.
///
/// Returns the backup path, or `None` when `db` is missing, empty, has no
/// `meta` table or holds another schema version; those are left untouched.
/// Refuses with [`StoreError::ControllerRunning`] when another process holds
/// the controller lock beside the database.
pub(super) fn retire_v1(db: &Path) -> Result<Option<PathBuf>, StoreError> {
    if !is_old_database(db)? {
        return Ok(None);
    }
    refuse_if_controller_runs(db)?;
    flush_wal(db)?;
    let bak = free_backup_name(db);
    fs::rename(db, &bak)?;
    for suffix in ["-wal", "-shm"] {
        match fs::remove_file(with_suffix(db, suffix)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    let _ = writeln!(
        std::io::stderr(),
        "moor: moved the old state database to {}; starting a fresh store",
        bak.display()
    );
    Ok(Some(bak))
}

/// True when `db` exists and records the old schema version.
fn is_old_database(db: &Path) -> Result<bool, StoreError> {
    match fs::metadata(db) {
        Ok(m) if m.len() > 0 => {}
        Ok(_) => return Ok(false),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e.into()),
    }
    let conn = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let has_meta: bool = conn.query_row(
        "SELECT count(*) > 0 FROM sqlite_master WHERE type = 'table' AND name = 'meta'",
        [],
        |r| r.get(0),
    )?;
    if !has_meta {
        return Ok(false);
    }
    let version: Option<String> = conn
        .query_row("SELECT v FROM meta WHERE k = 'schema_version'", [], |r| {
            r.get(0)
        })
        .optional()?;
    Ok(version.as_deref() == Some(OLD_VERSION))
}

/// Fails when a controller other than this process holds the lock file
/// beside `db`.
fn refuse_if_controller_runs(db: &Path) -> Result<(), StoreError> {
    let Some(dir) = db.parent() else {
        return Ok(());
    };
    let mut file = match File::open(dir.join("controller.sock.lock")) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    match file.try_lock() {
        Ok(()) => {
            file.unlock()?;
            Ok(())
        }
        Err(TryLockError::WouldBlock) => {
            let mut held = String::new();
            file.read_to_string(&mut held)?;
            let pid = held.lines().next().unwrap_or("?").to_owned();
            if pid == std::process::id().to_string() {
                Ok(())
            } else {
                Err(StoreError::ControllerRunning { pid })
            }
        }
        Err(TryLockError::Error(e)) => Err(e.into()),
    }
}

/// Folds the write-ahead log into the main file so the move carries every row.
fn flush_wal(db: &Path) -> Result<(), StoreError> {
    let conn = Connection::open(db)?;
    conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
    conn.close().map_err(|(_, e)| e)?;
    Ok(())
}

/// The first of `<db>.v1.bak`, `<db>.v1.bak.1`, `.2`, ... that does not exist.
fn free_backup_name(db: &Path) -> PathBuf {
    let base = with_suffix(db, ".v1.bak");
    if !base.exists() {
        return base;
    }
    (1..=u32::MAX)
        .map(|n| with_suffix(&base, &format!(".{n}")))
        .find(|p| !p.exists())
        .unwrap_or(base)
}

/// `path` with `suffix` appended to its file name.
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

#[cfg(test)]
mod tests;
