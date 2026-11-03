//! Stores for tests: on `SQLite` at the path a test names, or, when `MOOR_TEST_POSTGRES_URL`
//! names a Postgres database, in a schema of it that stands for that path, so opening the same
//! path again opens the same store.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use moor_core::seam::clock::Clock;
use postgres::{Client, NoTls};

use super::Store;

/// The variable naming the Postgres database store tests run on.
const POSTGRES_URL: &str = "MOOR_TEST_POSTGRES_URL";

/// How long a test's schema is kept before a later test drops it.
const KEEP_SECS: u64 = 3600;

/// A store for a test: the `SQLite` file `path`, or on Postgres the schema for `path`, with
/// `path`'s directory as its state dir.
pub(crate) fn open_store(path: &Path, clock: Arc<dyn Clock>) -> Store {
    let Ok(url) = std::env::var(POSTGRES_URL) else {
        return Store::open(path, clock).unwrap();
    };
    let mut config: postgres::Config = url.parse().unwrap();
    let mut admin = config.connect(NoTls).unwrap();
    #[expect(
        clippy::disallowed_methods,
        reason = "a schema's age is wall-clock time across test runs, not the store's clock"
    )]
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let name = schema_for(&mut admin, path, now);
    sweep(&mut admin, now);
    config.options(format!("-c search_path={name}").as_str());
    let state_dir = path.parent().unwrap_or(path);
    std::fs::create_dir_all(state_dir).unwrap();
    Store::open_postgres(config, state_dir, clock).unwrap()
}

/// The schema standing for `path`, made now if no test has made it yet.
fn schema_for(admin: &mut Client, path: &Path, now: u64) -> String {
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    let key = format!("{:016x}", hasher.finish());
    let made = admin
        .query_opt(
            "SELECT nspname::text FROM pg_namespace WHERE nspname LIKE $1",
            &[&format!("moor\\_store\\_test\\_%\\_{key}")],
        )
        .unwrap();
    if let Some(row) = made {
        return row.get(0);
    }
    let name = format!("moor_store_test_{now}_{key}");
    admin
        .batch_execute(&format!("CREATE SCHEMA IF NOT EXISTS {name}"))
        .unwrap();
    name
}

/// Drop the schemas tests made over `KEEP_SECS` ago. Another test dropping one at the same time
/// makes this drop fail, which is harmless.
fn sweep(admin: &mut Client, now: u64) {
    let names = admin
        .query(
            "SELECT nspname::text FROM pg_namespace WHERE nspname LIKE 'moor\\_store\\_test\\_%'",
            &[],
        )
        .unwrap();
    for row in names {
        let name: String = row.get(0);
        let made = name.split('_').nth(3).and_then(|t| t.parse::<u64>().ok());
        if made.is_some_and(|t| t.saturating_add(KEEP_SECS) < now) {
            drop(admin.batch_execute(&format!("DROP SCHEMA IF EXISTS {name} CASCADE")));
        }
    }
}
