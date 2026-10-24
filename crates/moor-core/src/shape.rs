//! How many CPUs and how much memory a Box has.

/// A Box's CPUs and memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shape {
    /// CPUs the Box can run on.
    pub cpus: u32,
    /// Memory in MiB.
    pub mem_mib: u64,
}
