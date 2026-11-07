//! A saved Box, as data.

use crate::domain::event::ProviderStep;
use crate::name::BoxHandle;

/// A saved copy of a Box's memory and disk that new Boxes can start from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkpoint {
    /// The provider's id for it.
    pub id: String,
    /// What it adds to the host's disk, when the provider reports it: for a delta over another
    /// checkpoint, only its own share.
    pub bytes: Option<u64>,
    /// The checkpoint it is a delta over, which cannot go while this one stays; none over the
    /// base image.
    pub parent: Option<String>,
}

/// A Box a fork started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Forked {
    /// The new Box.
    pub handle: BoxHandle,
    /// What the provider reports the fork spent its time on.
    pub steps: Vec<ProviderStep>,
}

/// The filesystem a provider keeps its checkpoints on, in MiB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckpointDisk {
    /// What is free.
    pub free_mib: u64,
    /// Its size.
    pub total_mib: u64,
}
