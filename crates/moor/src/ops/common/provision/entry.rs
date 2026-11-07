//! How a Box comes to want a machine.

use moor_core::domain::box_state::BoxLifecycleInput;
use moor_core::domain::decision::Field;

/// Where the Box stands when its machine is wanted.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Entry {
    /// A new Box still being checked.
    Fresh,
    /// A Box whose setup failed.
    Retry,
    /// A working Box that lost its machine; its state stays.
    Stay,
    /// A Box queued until a machine slot frees.
    Waiting,
    /// A Box being set up whose acquire a restart cut off, whose start goes on.
    Again,
    /// A Box being set up whose cut-off acquire goes on without it, which this process takes over.
    Adopt,
}

impl Entry {
    /// The Box input that starts the step, if any.
    pub(super) const fn begin(self) -> Option<BoxLifecycleInput> {
        match self {
            Self::Fresh => Some(BoxLifecycleInput::Admitted),
            Self::Waiting => Some(BoxLifecycleInput::Promoted),
            Self::Retry => Some(BoxLifecycleInput::Retry),
            Self::Stay | Self::Again | Self::Adopt => None,
        }
    }

    /// Whether the Box comes up from this step, rather than already working.
    pub(super) const fn sets_up(self) -> bool {
        !matches!(self, Self::Stay)
    }

    /// The state reason once the Box gets its machine: a queued Box no longer waits for room.
    pub(super) const fn reason_once_admitted(self) -> Field<String> {
        match self {
            Self::Waiting => Field::Clear,
            Self::Fresh | Self::Retry | Self::Stay | Self::Again | Self::Adopt => Field::Keep,
        }
    }
}
