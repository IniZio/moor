//! Box-side helper that publishes `.moor/proof/` files to the `moor-proof/<task>` branch.

#[cfg(test)]
mod tests;

/// Workdir-relative path the launcher installs [`SCRIPT`] at.
pub const FILE: &str = ".moor/publish-proof.sh";

/// POSIX sh text. Run as `sh .moor/publish-proof.sh <task>` from the workdir.
///
/// Commits the top-level regular files of `.moor/proof/` onto `moor-proof/<task>` with a
/// temporary index (HEAD, branch, worktree and index stay untouched), pushes it through the
/// `moor` remote as a fast-forward, and prints Markdown links pinned to the commit sha.
pub const SCRIPT: &str = include_str!("proof_publish/publish-proof.sh");
