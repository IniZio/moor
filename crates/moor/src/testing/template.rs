//! A migrated store file made once per test build and copied into each test's state dir.
//!
//! Opening a fresh store runs every migration, about 130 ms of SQL, and nextest starts a
//! process per test, so a once-per-process cache would save nothing. The template lives in the
//! target dir next to the test binary and is named for that binary's size and modification
//! time, so a rebuild makes a new template and removes the old ones.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use moor_core::seam::clock::{Clock, Ts};

use crate::store::{Database, Store, StoreError, STORE_FILE};
use crate::testing::clock::FakeClock;

/// The prefix of every template's file name.
const PREFIX: &str = "moor-test-store-";

/// Opens the controller's store in `state_dir` like [`Store::open_claimed`], but first puts a
/// migrated, empty db there when the store is a file the dir does not yet hold, so opening it
/// runs no migration. A template that cannot be made or copied leaves the dir as found, and
/// the store migrates as it would have.
pub(crate) fn open(
    state_dir: &Path,
    database: Option<&Database>,
    clock: &Arc<dyn Clock>,
) -> Result<Store, StoreError> {
    if database.is_none() {
        seed(&state_dir.join(STORE_FILE));
    }
    Store::open_claimed(state_dir, database, Arc::clone(clock))
}

/// Copies the template to `db` unless something is there already.
fn seed(db: &Path) {
    let dir = db.parent().expect("the db is in a state dir");
    if db.exists() || !dir.is_dir() {
        return;
    }
    let Ok(template) = template() else { return };
    // Copy beside the target and rename, so the store never opens a half-copied file.
    let part = dir.join(format!("{STORE_FILE}.seed"));
    if fs::copy(&template, &part).is_ok() && fs::rename(&part, db).is_err() {
        let _ = fs::remove_file(&part);
    }
}

/// The template file for this test binary, made first if there is none.
fn template() -> io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let built = fs::metadata(&exe)?;
    let nanos = built
        .modified()?
        .duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_nanos();
    let dir = exe
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| io::Error::other("the test binary has no target dir"))?;
    let name = format!("{PREFIX}{:x}-{nanos:x}.db", built.len());
    let path = dir.join(&name);
    if !path.exists() {
        build(dir, &path)?;
        forget_others(dir, &name);
    }
    Ok(path)
}

/// Migrates a store in a scratch dir under `dir` and renames its db to `path`. Concurrent
/// test processes may each build one; the files are the same, so the last rename wins.
fn build(dir: &Path, path: &Path) -> io::Result<()> {
    let scratch = tempfile::tempdir_in(dir)?;
    let db = scratch.path().join(STORE_FILE);
    let clock = Arc::new(FakeClock::new(Ts::from_millis(1000)));
    drop(Store::open(&db, clock).map_err(io::Error::other)?);
    // Dropping the last connection folds the WAL into the db; a template that still has one
    // would lose rows when only the db is copied.
    if db.with_file_name(format!("{STORE_FILE}-wal")).exists() {
        return Err(io::Error::other("the template store left a WAL file"));
    }
    fs::rename(&db, path)
}

/// Removes the templates of earlier builds from `dir`, keeping the one named `keep`.
fn forget_others(dir: &Path, keep: &str) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(PREFIX) && name != keep {
            let _ = fs::remove_file(entry.path());
        }
    }
}
