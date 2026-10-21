//! Each person's agent credential: one owner-only file per agent and Slack user under the
//! state dir, and each user's own config dir for that agent. What a credential looks like
//! and how to check it belongs to the agent, behind [`CredentialCheck`].

use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Write as _};
use std::os::unix::fs::{
    DirBuilderExt as _, MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _,
};
use std::path::{Path, PathBuf};

use moor_core::secret::Secret;

/// The agent a credential belongs to; its name is a path component and a command word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentKind {
    /// Claude Code, with the token `claude setup-token` prints.
    Claude,
}

impl AgentKind {
    /// The agent a command means when it names none.
    pub(crate) const DEFAULT: Self = Self::Claude;

    /// Every agent a person can link a credential for.
    const ALL: [Self; 1] = [Self::Claude];

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

    /// What the login modal asks this agent's user to paste.
    pub(crate) const fn login_prompt(self) -> &'static str {
        match self {
            Self::Claude => crate::coordinator::claude::LOGIN_PROMPT,
        }
    }

    /// The agent called `name`.
    pub(crate) fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.name() == name)
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

/// The directory that holds every user's credential for `agent`.
fn agent_dir(state_dir: &Path, agent: AgentKind) -> PathBuf {
    state_dir.join("credentials").join(agent.name())
}

/// The file that holds `user`'s credential for `agent`.
pub(crate) fn credential_path(state_dir: &Path, agent: AgentKind, user: &SlackUser) -> PathBuf {
    agent_dir(state_dir, agent).join(user.as_str())
}

/// Writes `credential` as `user`'s file for `agent`, mode 0600 in 0700 directories. It
/// writes a temp file beside it and renames it over the old one, so a reader sees the old
/// credential or the new one, never part of one.
pub(crate) fn store(
    state_dir: &Path,
    agent: AgentKind,
    user: &SlackUser,
    credential: &Secret,
) -> Result<(), String> {
    let path = credential_path(state_dir, agent, user);
    let dir = agent_dir(state_dir, agent);
    let parent = state_dir.join("credentials");
    private_dir(&parent, daemon_uid()).map_err(|e| format!("{}: {e}", parent.display()))?;
    private_dir(&dir, daemon_uid()).map_err(|e| format!("{}: {e}", dir.display()))?;
    let temp = dir.join(format!(".{}.tmp", user.as_str()));
    let write = || -> io::Result<()> {
        remove_if_present(&temp)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)?;
        file.write_all(credential.expose())?;
        file.sync_all()?;
        fs::rename(&temp, &path)
    };
    write().map_err(|e| {
        let _ = remove_if_present(&temp);
        format!("{}: {e}", path.display())
    })
}

/// Reads `user`'s credential for `agent`; `None` when the user linked none. The directories
/// and the file must be this user's own: no symlink, no other owner, and no group or other
/// bit on the file. Anything else is an error, so a planted file is never read.
pub(crate) fn load(
    state_dir: &Path,
    agent: AgentKind,
    user: &SlackUser,
) -> Result<Option<Secret>, String> {
    let path = credential_path(state_dir, agent, user);
    let shown = |p: &Path, e: &dyn std::fmt::Display| format!("{}: {e}", p.display());
    let owner = daemon_uid();
    for dir in [state_dir.join("credentials"), agent_dir(state_dir, agent)] {
        match owned(&dir, owner, true) {
            Ok(true) => {}
            Ok(false) => return Ok(None),
            Err(e) => return Err(shown(&dir, &e)),
        }
    }
    match owned(&path, owner, false) {
        Ok(true) => {}
        Ok(false) => return Ok(None),
        Err(e) => return Err(shown(&path, &e)),
    }
    let bytes = fs::read(&path).map_err(|e| shown(&path, &e))?;
    let text = String::from_utf8_lossy(&bytes);
    Ok(Some(Secret::new(text.trim().as_bytes().to_vec())))
}

/// Whether `path` is there; an error when it is a symlink, the wrong kind, not owned by
/// `owner`, or, for a file, open to group or others.
fn owned(path: &Path, owner: u32, dir: bool) -> io::Result<bool> {
    let meta = match fs::symlink_metadata(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        other => other?,
    };
    let kind_ok = if dir { meta.is_dir() } else { meta.is_file() };
    let shared = !dir && meta.mode() & 0o077 != 0;
    if kind_ok && !shared && meta.uid() == owner {
        return Ok(true);
    }
    let why = "not a private file or directory that this user owns";
    Err(io::Error::new(io::ErrorKind::PermissionDenied, why))
}

/// Deletes `user`'s credential for `agent`; a user with no file is already unlinked.
pub(crate) fn remove(state_dir: &Path, agent: AgentKind, user: &SlackUser) -> Result<(), String> {
    let path = credential_path(state_dir, agent, user);
    remove_if_present(&path).map_err(|e| format!("{}: {e}", path.display()))
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

/// Deletes `path`; a missing file is not an error.
fn remove_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

#[cfg(test)]
mod tests;
