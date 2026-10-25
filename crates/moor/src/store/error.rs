//! How a store operation fails.

use std::path::PathBuf;

use moor_core::{BoxId, BoxName};

/// Store failure.
#[derive(Debug, thiserror::Error)]
pub(crate) enum StoreError {
    /// `SQLite` failed.
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// An event payload failed to (de)serialise.
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    /// The state directory or a database file could not be prepared.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// The file predates migrations at a version this build cannot adopt; it is left as found.
    #[error(
        "the state database {} has schema version `{found}` and this moor adopts only `{}`; \
         use a fresh state dir: set MOOR_STATE_DIR to a new directory, or move the old one away \
         (moor does not delete it)",
        db.display(),
        super::init::BASELINE_VERSION
    )]
    Schema {
        /// The database file.
        db: PathBuf,
        /// Version found in `meta`.
        found: String,
    },
    /// The file has migrations this build does not know, or an applied one changed.
    #[error("the state database {} cannot be migrated by this moor: {source}", db.display())]
    Migration {
        /// The database file.
        db: PathBuf,
        /// What the migration run refused.
        source: moor_migrate::Error,
    },
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
