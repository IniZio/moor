//! The `surface:` section of the user config: what holds Box agent panes.

use serde::Deserialize;

/// `surface.herdr:` section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct HerdrConfig {
    /// herdr session name.
    pub session: Option<String>,
}

/// `surface:` section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct SurfaceConfig {
    /// Which surface holds Box agent panes.
    pub kind: SurfaceKind,
    /// herdr settings.
    pub herdr: HerdrConfig,
}

/// `surface.kind`: what holds Box agent panes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SurfaceKind {
    /// A herdr session.
    #[default]
    Herdr,
    /// Pseudo-terminals inside the controller process.
    Pty,
}
