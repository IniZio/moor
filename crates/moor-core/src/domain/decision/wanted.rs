//! What moor wants a Box to end as, and the external step in flight for it. The Box's owner
//! writes both under its turn, so a restart reads them instead of the Box's history.

use crate::name::RunId;

/// What moor should end a Box as once it can. A close outranks a suspend, which outranks a
/// sleep, so two ops that want different ends meet at the stronger one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Wanted {
    /// Sleep once the Box is quiet.
    Sleep,
    /// Suspend mid-work to relieve the node.
    Suspend {
        /// Why.
        reason: String,
    },
    /// Close.
    Close {
        /// Whether the close stops a start or a stuck release rather than wait for it.
        force: bool,
        /// Why moor closes it, when a person did not ask.
        reason: Option<String>,
    },
}

/// A [`Wanted`] without its details, ordered by rank: `Sleep < Suspend < Close`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WantedKind {
    /// [`Wanted::Sleep`].
    Sleep,
    /// [`Wanted::Suspend`].
    Suspend,
    /// [`Wanted::Close`].
    Close,
}

impl WantedKind {
    /// The stored name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sleep => "sleep",
            Self::Suspend => "suspend",
            Self::Close => "close",
        }
    }
}

impl Wanted {
    /// Its kind, which carries its rank.
    #[must_use]
    pub const fn kind(&self) -> WantedKind {
        match self {
            Self::Sleep => WantedKind::Sleep,
            Self::Suspend { .. } => WantedKind::Suspend,
            Self::Close { .. } => WantedKind::Close,
        }
    }

    /// Whether this replaces `held` on the Box: a higher rank does, as does a forced close over
    /// one that was not forced.
    #[must_use]
    pub fn outranks(&self, held: Option<&Self>) -> bool {
        let Some(held) = held else { return true };
        match (self, held) {
            (Self::Close { force: true, .. }, Self::Close { force: false, .. }) => true,
            _ => self.kind() > held.kind(),
        }
    }

    /// The wanted action its stored columns hold; `None` for a kind this build does not know.
    #[must_use]
    pub fn from_columns(kind: &str, force: bool, reason: Option<String>) -> Option<Self> {
        match kind {
            "sleep" => Some(Self::Sleep),
            "suspend" => Some(Self::Suspend {
                reason: reason.unwrap_or_default(),
            }),
            "close" => Some(Self::Close { force, reason }),
            _ => None,
        }
    }

    /// Whether it is a forced close.
    #[must_use]
    pub const fn force(&self) -> bool {
        matches!(self, Self::Close { force: true, .. })
    }

    /// Its reason, if it has one.
    #[must_use]
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Sleep => None,
            Self::Suspend { reason } => Some(reason),
            Self::Close { reason, .. } => reason.as_deref(),
        }
    }
}

/// The external step in flight for a Box, which a process that stops mid-step leaves behind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// Asking the provider for a machine.
    Acquire,
    /// Running the setup lines and the steps after them.
    Setup,
    /// Starting the agent.
    Launch,
    /// A command running in the Box.
    Run {
        /// The run.
        run: RunId,
    },
    /// Releasing the machine of a closing Box.
    Release,
}

/// What the next process does with an effect the last one left in flight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Restart {
    /// Do it again: the step is safe to repeat, or checks first that it did not finish.
    Rerun,
    /// Record that it was cut; doing it twice could repeat what it did.
    Report,
}

impl Effect {
    /// The stored name.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Acquire => "acquire",
            Self::Setup => "setup",
            Self::Launch => "launch",
            Self::Run { .. } => "run",
            Self::Release => "release",
        }
    }

    /// The run of a run effect.
    #[must_use]
    pub const fn run(&self) -> Option<RunId> {
        match self {
            Self::Run { run } => Some(*run),
            Self::Acquire | Self::Setup | Self::Launch | Self::Release => None,
        }
    }

    /// What a restart does with it. A launch reruns unless its agent lives, which the rerun
    /// probes first; a run's command may have changed the Box, so it is reported.
    #[must_use]
    pub const fn on_restart(&self) -> Restart {
        match self {
            Self::Acquire | Self::Setup | Self::Launch | Self::Release => Restart::Rerun,
            Self::Run { .. } => Restart::Report,
        }
    }

    /// The effect its stored columns hold; `None` for a kind this build does not know, or a run
    /// with no run.
    #[must_use]
    pub fn from_columns(kind: &str, run: Option<RunId>) -> Option<Self> {
        match kind {
            "acquire" => Some(Self::Acquire),
            "setup" => Some(Self::Setup),
            "launch" => Some(Self::Launch),
            "run" => run.map(|run| Self::Run { run }),
            "release" => Some(Self::Release),
            _ => None,
        }
    }
}

/// A change to what moor wants a Box to end as.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum WantChange {
    /// Leave it.
    #[default]
    Keep,
    /// Want this, unless the Box already wants something that outranks it.
    Raise(Wanted),
    /// The wanted action of this kind is done or given up; any other stays.
    Done(WantedKind),
}

/// A change to the effect in flight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EffectChange {
    /// Leave it.
    #[default]
    Keep,
    /// Begin this effect; the Box must have none in flight.
    Begin(Effect),
    /// The effect in flight, if any, ended, and this one begins: a start's acquire hands over to
    /// its setup. A run in flight is never replaced, as only a restart may end it unfinished.
    Next(Effect),
    /// Try the effect in flight once more.
    Again,
    /// The effect in flight ended.
    End,
    /// The effect in flight ended when it is one of this kind; another stays.
    Finish(Effect),
}

impl super::Decision {
    /// Sets the change to what moor wants the Box to end as.
    #[must_use]
    pub fn want(mut self, c: WantChange) -> Self {
        self.wanted = c;
        self
    }

    /// Sets the change to the effect in flight.
    #[must_use]
    pub const fn effect(mut self, c: EffectChange) -> Self {
        self.effect = c;
        self
    }
}
