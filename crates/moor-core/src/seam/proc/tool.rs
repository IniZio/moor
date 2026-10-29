//! The external programs moor may run.

/// An external program moor may run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    /// The crabbox driver hub.
    Crabbox,
    /// The herdr surface.
    Herdr,
    /// The agent engine program (claude today).
    Agent,
    /// OpenSSH client.
    Ssh,
    /// GitHub CLI.
    Gh,
    /// Git.
    Git,
    /// Rsync.
    Rsync,
    /// This binary, re-invoked.
    MoorSelf,
    /// POSIX shell, for credential `cmd` sources.
    Sh,
}

impl Tool {
    /// Environment variable that overrides the program path.
    #[must_use]
    pub const fn bin_var(self) -> &'static str {
        match self {
            Self::Crabbox => "MOOR_CRABBOX_BIN",
            Self::Herdr => "MOOR_HERDR_BIN",
            Self::Agent => "MOOR_CLAUDE_BIN",
            Self::Ssh => "MOOR_SSH_BIN",
            Self::Gh => "MOOR_GH_BIN",
            Self::Git => "MOOR_GIT_BIN",
            Self::Rsync => "MOOR_RSYNC_BIN",
            Self::MoorSelf => "MOOR_SELF_BIN",
            Self::Sh => "MOOR_SH_BIN",
        }
    }

    /// Default program name looked up on `PATH`.
    #[must_use]
    pub const fn program(self) -> &'static str {
        match self {
            Self::Crabbox => "crabbox",
            Self::Herdr => "herdr",
            Self::Agent => "claude",
            Self::Ssh => "ssh",
            Self::Gh => "gh",
            Self::Git => "git",
            Self::Rsync => "rsync",
            Self::MoorSelf => "moor",
            Self::Sh => "sh",
        }
    }
}
