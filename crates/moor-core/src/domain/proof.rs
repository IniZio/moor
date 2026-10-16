//! Proof file record shared by the view and the proof decisions.

use serde::{Deserialize, Serialize};

/// One proof file found in a Box.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProofFile {
    /// Path relative to the proof directory, `/`-separated.
    pub name: String,
    /// Size in bytes.
    pub size: u64,
    /// Lowercase hex SHA-256 of the contents.
    pub sha256: String,
}
