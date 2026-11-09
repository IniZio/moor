//! What each op may do on a Box, given what is already in flight there.

/// What an op asks its Box's owner to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Cmd {
    /// Start a Box: acquire its machine and run setup.
    Start,
    /// Start a failed Box again.
    Retry,
    /// Carry on a start a restart cut short.
    Resume,
    /// Record what the agent did since the last pass.
    Settle,
    /// Wake an asleep Box.
    Wake,
    /// Put a Box to sleep.
    Sleep,
    /// Suspend a Box to free memory for another.
    Suspend,
    /// Close a Box.
    Close {
        /// Whether a start in flight is stopped rather than waited for.
        force: bool,
    },
    /// Send the agent a prompt.
    Steer,
    /// Launch the agent again.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "a relaunch runs under its start's or steer's turn"
        )
    )]
    Relaunch,
    /// Run a command in the Box.
    Run,
    /// Run a command in the Box with its stdio streamed, keeping no run record.
    Exec,
    /// Record what a fresh agent's first moments showed.
    Watch,
    /// Bring the row in line with the machine.
    Reconcile,
    /// Promote the Box's work to its links.
    Promote,
    /// Copy files to or from the Box.
    Copy,
    /// Open or end a session on the Box.
    Session,
}

impl Cmd {
    /// The op's name in a busy message.
    pub(super) const fn name(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Retry => "retry",
            Self::Resume => "resume",
            Self::Settle => "settle",
            Self::Wake => "wake",
            Self::Sleep => "sleep",
            Self::Suspend => "suspend",
            Self::Close { .. } => "close",
            Self::Steer => "steer",
            Self::Relaunch => "relaunch",
            Self::Run => "run",
            Self::Exec => "exec",
            Self::Watch => "watch",
            Self::Reconcile => "reconcile",
            Self::Promote => "promote",
            Self::Copy => "copy",
            Self::Session => "session",
        }
    }

    /// Whether the op acquires a machine or runs setup, which can take minutes.
    const fn is_long(self) -> bool {
        matches!(self, Self::Start | Self::Retry | Self::Resume)
    }
}

/// How the owner answers a call, given what is in flight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Admit {
    /// Take the turn.
    Now,
    /// Wait for the turn in arrival order.
    Queue,
    /// Refuse with this message.
    Refuse(&'static str),
    /// Refuse as busy with the holder's op.
    Busy,
    /// Stop the holder and take the turn next.
    Preempt,
}

/// The admission policy: `cmd` arriving while `holder` has the turn and a run may be in flight.
///
/// [`super::Owners::try_turn`] skips any Box with something in flight, so this table is for user ops.
pub(super) const fn admit(cmd: Cmd, holder: Option<Cmd>, run: bool) -> Admit {
    match (holder, cmd) {
        (Some(h), Cmd::Close { force: true }) if h.is_long() => Admit::Preempt,
        (Some(h), _) if h.is_long() => Admit::Busy,
        (Some(_), _) => Admit::Queue,
        (None, Cmd::Sleep | Cmd::Close { force: false } | Cmd::Run) if run => {
            Admit::Refuse("box busy: run in progress")
        }
        (None, Cmd::Close { force: true }) if run => Admit::Queue,
        (None, _) => Admit::Now,
    }
}
