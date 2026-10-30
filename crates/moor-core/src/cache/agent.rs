//! The agent's install directory as a cache volume, kept per user, so a later Box of the same
//! user finds the agent already downloaded whatever repo it serves.

use super::{CacheName, CachePath, OnFail, Volume};

/// The volume's name.
pub const NAME: &str = "agent";

/// What the agent adapter says to keep.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCache {
    /// The directories that hold the install, in `~/` form.
    pub dirs: Vec<String>,
    /// POSIX sh text that exits 0 only when what the directories hold is a whole install.
    pub check: String,
}

/// The agent volume; none when a directory is not a valid cache path.
pub(super) fn volume(agent: &AgentCache) -> Option<Volume> {
    let paths = agent
        .dirs
        .iter()
        .map(|d| CachePath::parse(d))
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    Some(Volume {
        name: CacheName::parse(NAME).ok()?,
        paths,
        size_gb: None,
        seal: vec![agent.check.clone()],
        on_fail: OnFail::Clear,
    })
}
