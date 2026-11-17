//! What a provider can do, as data.

/// Facts about a provider, as data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DriverCaps {
    /// Whether the Box reports a working directory.
    pub has_workdir: bool,
    /// Whether the provider keeps cache volumes across Boxes.
    pub cache_volumes: bool,
}
