//! Retires a state database written by the old table layout.
//!
//! There is no migration: the old file is moved aside and a fresh store is
//! created in its place. The move never deletes anything: the `-wal` and
//! `-shm` files go beside the backup, and no existing backup is overwritten.
//! A lock file beside the database serialises the version check and the move,
//! so two processes cannot both retire the same file or move a fresh store
//! aside.

use std::fs::{self, File, TryLockError};
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags, OptionalExtension};

use super::StoreError;

mod files;

/// The `schema_version` value written by the old table layout.
const OLD_VERSION: &str = "1";

/// How many backup names to try when another process takes the chosen one.
const BACKUP_ATTEMPTS: u32 = 64;

/// The outcome of [`retire_v1`]. Dropping it releases the retire lock, so
/// keep it until the fresh store is created.
#[must_use]
pub(super) struct Retired {
    /// Where the old database went, or `None` when nothing was moved.
    pub(super) backup: Option<PathBuf>,
    /// Exclusive lock on the retire lock file, released on drop.
    _lock: File,
}

/// Moves a database with the old table layout to `<db>.v1.bak` (or the first
/// free numbered name) so a fresh store can be created at `db`. Its `-wal` and
/// `-shm` files move to `<backup>-wal` and `<backup>-shm`.
///
/// Takes the retire lock first and checks the version under it, so the second
/// of two racing callers sees the fresh store and moves nothing. The returned
/// [`Retired`] holds that lock.
///
/// `backup` is `None` when `db` is missing, empty, has no `meta` table or
/// holds another schema version; those are left untouched. Refuses with
/// [`StoreError::ControllerRunning`] when another process holds the controller
/// lock beside the database, and with [`StoreError::StoreInUse`] when a
/// reader or writer stops the log being folded into the main file; the
/// database is untouched in both cases.
pub(super) fn retire_v1(db: &Path) -> Result<Retired, StoreError> {
    let lock = files::lock_retire(db)?;
    let mut retired = Retired {
        backup: None,
        _lock: lock,
    };
    if !is_old_database(db)? {
        return Ok(retired);
    }
    refuse_if_controller_runs(db)?;
    flush_wal(db)?;
    let bak = move_aside(db)?;
    let _ = writeln!(
        std::io::stderr(),
        "moor: moved the old state database to {}; starting a fresh store",
        bak.display()
    );
    retired.backup = Some(bak);
    Ok(retired)
}

/// Moves `db` to a free backup name and its `-wal`/`-shm` beside it.
fn move_aside(db: &Path) -> Result<PathBuf, StoreError> {
    for attempt in 0..BACKUP_ATTEMPTS {
        let bak = backup_name(db, attempt);
        match files::move_noreplace(db, &bak) {
            Ok(()) => {
                for suffix in ["-wal", "-shm"] {
                    match files::move_noreplace(
                        &with_suffix(db, suffix),
                        &with_suffix(&bak, suffix),
                    ) {
                        Ok(()) => {}
                        Err(e) if e.kind() == ErrorKind::NotFound => {}
                        Err(e) => return Err(e.into()),
                    }
                }
                return Ok(bak);
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
    }
    Err(std::io::Error::new(ErrorKind::AlreadyExists, "no free backup name").into())
}

/// True when `db` exists and records the old schema version.
fn is_old_database(db: &Path) -> Result<bool, StoreError> {
    match fs::metadata(db) {
        Ok(m) if m.len() > 0 => {}
        Ok(_) => return Ok(false),
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(false),
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
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
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
///
/// Refuses with [`StoreError::StoreInUse`] unless the checkpoint ran to
/// completion; an open reader or a later commit leaves frames in the log, and
/// the original files are left as they were.
fn flush_wal(db: &Path) -> Result<(), StoreError> {
    let conn = Connection::open(db)?;
    conn.busy_timeout(std::time::Duration::ZERO)?;
    let (busy, log, checkpointed): (i64, i64, i64) =
        conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?;
    if busy != 0 || log != checkpointed {
        return Err(StoreError::StoreInUse);
    }
    conn.close().map_err(|(_, e)| e)?;
    Ok(())
}

/// `<db>.v1.bak` for attempt 0, then `<db>.v1.bak.1`, `.2`, ...
fn backup_name(db: &Path, attempt: u32) -> PathBuf {
    let base = with_suffix(db, ".v1.bak");
    if attempt == 0 {
        base
    } else {
        with_suffix(&base, &format!(".{attempt}"))
    }
}

/// `path` with `suffix` appended to its file name.
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

#[cfg(test)]
mod tests;
