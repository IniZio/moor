//! The credential record an admission check reports.

use serde::Serialize;

/// One credential an admission check looked at; never carries a value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheckedCred {
    /// The credential name.
    pub name: String,
    /// The config layer that defines it.
    pub source: String,
    /// Whether it resolved.
    pub resolves: bool,
}
