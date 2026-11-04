//! How a database operation fails, in terms every backend reports.

use super::{Cause, Kind, Result, UniqueKey};

/// Why a database operation failed.
#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    /// A query read for one row returned none.
    #[error("the query returned no rows")]
    NoRows,
    /// The row has no such column.
    #[error("no column {0}")]
    NoColumn(String),
    /// A column held a storage class the reader does not take.
    #[error("column {index} `{name}` holds {found:?}, not the type read")]
    Type {
        /// The column's position.
        index: usize,
        /// The column's name.
        name: String,
        /// What it held.
        found: Kind,
    },
    /// A column held an integer out of the reader's range.
    #[error("column {index}: {value} is out of range")]
    OutOfRange {
        /// The column's position.
        index: usize,
        /// The integer it held.
        value: i64,
    },
    /// A column held a value its reader could not convert.
    #[error("column {index}: {source}")]
    Convert {
        /// The column's position.
        index: usize,
        /// Why it did not convert.
        source: Cause,
    },
    /// A statement's value cannot be bound.
    #[error("cannot bind {0}")]
    Bind(String),
    /// A write collided with a UNIQUE key.
    #[error("{message}")]
    Unique {
        /// The key it collided with.
        key: UniqueKey,
        /// The backend's message.
        message: String,
    },
    /// The backend failed.
    #[error("{0}")]
    Backend(Cause),
}

impl Error {
    /// The UNIQUE key a write collided with; none for any other error.
    pub(crate) const fn unique_key(&self) -> Option<&UniqueKey> {
        match self {
            Self::Unique { key, .. } => Some(key),
            _ => None,
        }
    }
}

/// Turns a query that found no row into `None`.
pub(crate) trait OptionalExtension<T> {
    /// `None` for [`Error::NoRows`], the value or other error as it was.
    fn optional(self) -> Result<Option<T>>;
}

impl<T> OptionalExtension<T> for Result<T> {
    fn optional(self) -> Result<Option<T>> {
        match self {
            Ok(v) => Ok(Some(v)),
            Err(Error::NoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }
}
