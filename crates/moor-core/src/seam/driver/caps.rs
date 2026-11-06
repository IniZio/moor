//! What a provider can do, as data.

/// How a provider keeps an idle Box cheap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SleepKind {
    /// The Driver suspends the Box.
    Suspend,
    /// The provider sleeps the Box on its own.
    Native,
    /// The Box cannot sleep.
    None,
}

/// Facts about a provider, as data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent provider facts, each a yes or no"
)]
pub struct DriverCaps {
    /// Sleep style.
    pub sleep: SleepKind,
    /// Whether a sleeping Box can wake.
    pub wake: bool,
    /// Box-to-host tunnel support.
    pub reverse_tunnel: bool,
    /// Whether the Box reports a working directory.
    pub has_workdir: bool,
    /// Whether the provider keeps cache volumes across Boxes.
    pub cache_volumes: bool,
    /// The provider can save cache volumes while a Box sleeps.
    pub cache_save_on_pause: bool,
    /// Whether the provider saves checkpoints and starts Boxes from them.
    pub checkpoints: bool,
}
