//! Each person's agent credential: what it looks like and how to check it before the front
//! stores it in the vault belongs to the agent, behind [`CredentialCheck`]. Each user also has
//! their own config dir for that agent under the state dir.

use std::fs::{self, DirBuilder};
use std::io;
use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

use moor_core::secret::Secret;

/// The agent a credential belongs to; its name is a path component.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentKind {
    /// Claude Code, with the token `claude setup-token` prints.
    Claude,
}

impl AgentKind {
    /// The agent's name, such as `claude`.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
        }
    }

    /// The agent's name in a sentence, such as `Claude`.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
        }
    }
}

/// Why a check did not accept a credential.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CheckError {
    /// The agent answered with an error; the text is its first line.
    Refused(String),
    /// The check could not ask the agent at all; the text says why.
    Unavailable(String),
}

/// How one agent checks a credential before the front stores it.
pub(crate) trait CredentialCheck {
    /// Why `credential` cannot be this agent's credential, found without asking the agent.
    fn check_format(&self, credential: &Secret) -> Result<(), String>;

    /// Asks the agent once whether it takes `credential`, with `config_dir` as the agent's
    /// own config dir for this user.
    fn check(&self, config_dir: &Path, credential: &Secret) -> Result<(), CheckError>;
}

/// A Slack user id checked to be safe as one path component: `U` or `W`, then 1 to 31
/// capital letters or digits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SlackUser(String);

impl SlackUser {
    /// The id, when it has the shape of a Slack user id.
    pub(crate) fn parse(id: &str) -> Option<Self> {
        let mut bytes = id.bytes();
        let first_ok = matches!(bytes.next(), Some(b'U' | b'W'));
        let rest_ok = (2..=32).contains(&id.len())
            && bytes.all(|b| b.is_ascii_uppercase() || b.is_ascii_digit());
        (first_ok && rest_ok).then(|| Self(id.to_owned()))
    }

    /// The id.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// `user`'s own config dir for `agent`, created mode 0700 when missing.
pub(crate) fn config_dir(
    state_dir: &Path,
    agent: AgentKind,
    user: &SlackUser,
) -> Result<PathBuf, String> {
    let parent = state_dir.join(agent.name());
    let dir = parent.join(user.as_str());
    for dir in [&parent, &dir] {
        private_dir(dir, daemon_uid()).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    Ok(dir)
}

/// The user id the daemon runs as.
pub(crate) fn daemon_uid() -> u32 {
    rustix::process::geteuid().as_raw()
}

/// Creates `dir` mode 0700, or checks the one there, then leaves it mode 0700. A symlink, a
/// file, or a directory that `owner` does not own is refused and left as it is.
pub(crate) fn private_dir(dir: &Path, owner: u32) -> io::Result<()> {
    match DirBuilder::new().mode(0o700).create(dir) {
        Err(e) if e.kind() != io::ErrorKind::AlreadyExists => return Err(e),
        _ => {}
    }
    let meta = fs::symlink_metadata(dir)?;
    if !meta.is_dir() || meta.uid() != owner {
        let why = "not a directory that this user owns";
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, why));
    }
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
}

#[cfg(test)]
mod tests;
