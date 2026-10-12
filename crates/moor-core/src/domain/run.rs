//! Run vocabulary shared by the driver seam and the run views.

/// Whether the worktree is synced into the Box before a command runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SyncMode {
    /// Sync the worktree first.
    Full,
    /// Run without syncing.
    None,
}
