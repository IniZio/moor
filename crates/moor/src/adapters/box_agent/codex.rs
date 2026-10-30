//! The Codex agent: what moor writes into `~/.codex` and `~/.agents` from a profile.

mod profile;

/// Writes a profile for Codex.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct Codex;
