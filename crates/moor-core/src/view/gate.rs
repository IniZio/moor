//! A Box's gates as `show` reports them.

use crate::gate::{status, GateLine, GateStatus, Gates};

/// The gates of a Box and where it stands against them.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct GateView {
    /// The gates' names, in the order they pass.
    pub gates: Vec<String>,
    /// Where the Box stands.
    pub status: GateStatus,
}

impl GateView {
    /// The view of `gates` after `lines`, oldest first.
    #[must_use]
    pub fn of(gates: &Gates, lines: &[GateLine]) -> Self {
        Self {
            gates: gates.iter().map(|g| g.name.to_string()).collect(),
            status: status(gates, lines),
        }
    }
}
