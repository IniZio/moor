//! How a store operation fails.

use std::path::PathBuf;

use moor_core::{BoxId, BoxName};

/// Store failure.
#[derive(Debug, thiserror::Error)]
pub(crate) enum StoreError {
    /// The database failed.
    #[error("database: {0}")]
    Db(#[from] crate::store::db::Error),
    /// An event payload failed to (de)serialise.
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    /// The state directory or a database file could not be prepared.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// The file is from an older moor at a schema this build cannot adopt; it is left as found.
    #[error(
        "the state database {db} is at {found}, older than this moor adopts; \
         use a fresh state dir: set MOOR_STATE_DIR to a new directory, or move the old one away \
         (moor does not delete it)"
    )]
    Schema {
        /// The database file.
        db: String,
        /// The schema it is at: a `meta` version, or the last migration it records.
        found: String,
    },
    /// The file has migrations this build does not know, or an applied one changed.
    #[error("the state database {db} cannot be migrated by this moor: {source}")]
    Migration {
        /// The database: its file, or its URL without the password.
        db: String,
        /// What the migration run refused.
        source: moor_migrate::Error,
    },
    /// The db needs migrating and a running controller owns its state dir.
    #[error(
        "the state database {} is older than this moor, and the controller running on its \
         state dir (pid {pid}) owns it; set MOOR_CONTROLLER={socket} to talk to that \
         controller, or stop it before running this moor on the state dir",
        db.display()
    )]
    Owned {
        /// The database file.
        db: PathBuf,
        /// The running controller's pid, as its lock file records it.
        pid: String,
        /// The running controller's socket, as its lock file records it.
        socket: String,
    },
    /// `MOOR_DATABASE_URL` does not name a database moor can open.
    #[error("MOOR_DATABASE_URL: {0}")]
    Url(String),
    /// The Postgres database did not answer when the store opened.
    #[error("cannot reach the store database {at}: {source}")]
    Unreachable {
        /// The database, without its password.
        at: String,
        /// Why the connection failed.
        source: crate::store::db::Error,
    },
    /// `moor store copy` refused its source or its target.
    #[error("{0}")]
    Copy(String),
    /// A box input or box input was illegal in the current state.
    #[error("{0}")]
    Illegal(#[from] moor_core::domain::Illegal),
    /// No such box.
    #[error("box {} not found", .0.get())]
    NotFound(BoxId),
    /// The decision or request was malformed.
    #[error("invalid: {0}")]
    Invalid(String),
    /// A sleep was refused because other Sessions are still working.
    #[error("sleep blocked: {busy} Sessions busy")]
    SleepBlocked {
        /// How many other Sessions are not idle.
        busy: usize,
    },
    /// A stored value could not be parsed.
    #[error("corrupt store: {0}")]
    Corrupt(String),
    /// An open box already has this name.
    #[error("name `{}` is taken by an open box", .0.as_str())]
    NameTaken(BoxName),
    /// An open box already works on this branch.
    #[error("branch `{0}` belongs to another open box")]
    BranchTaken(String),
    /// A user secret did not seal or open.
    #[error(transparent)]
    Vault(#[from] crate::vault::VaultError),
}
