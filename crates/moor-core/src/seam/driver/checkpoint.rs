//! A saved Box, as data.

/// A saved copy of a Box's memory and disk that new Boxes can start from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkpoint {
    /// The provider's id for it.
    pub id: String,
    /// Its size on the host, when the provider reports one.
    pub bytes: Option<u64>,
}

/// The filesystem a provider keeps its checkpoints on, in MiB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckpointDisk {
    /// What is free.
    pub free_mib: u64,
    /// Its size.
    pub total_mib: u64,
}
