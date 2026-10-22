//! What the Driver reports about a run detached in the Box.

pub use crate::boxscript::detached::{End as RunEnd, Outcome as RunOutcome};

/// What `Driver::launch_run` did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Launched {
    /// The run started; `pid` leads its process group in the Box.
    Started {
        /// The group leader's pid.
        pid: u32,
    },
    /// The run id was launched before, so nothing started.
    AlreadyLaunched,
}

/// Why `Driver::follow_run` stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FollowEnd {
    /// The run exited and its whole log was delivered.
    Exited,
    /// The run's process is gone and left no exit code.
    Lost,
    /// The channel to the Box broke or the deadline passed; the run may still be going.
    Cut,
}

/// Where `Driver::follow_run` stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Followed {
    /// The log offset after the last byte written out; a later follow resumes from it.
    pub offset: u64,
    /// Why it stopped.
    pub end: FollowEnd,
}

/// What `Driver::cancel_run` did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cancelled {
    /// The run's process group was signalled and is gone.
    Signalled,
    /// The run has no pid safe to signal, so nothing was sent.
    NoProcess,
}
