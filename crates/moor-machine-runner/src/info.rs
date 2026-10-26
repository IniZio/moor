//! What `machine_info` returns.

use serde::Serialize;

use crate::hooks::Facts;

/// Where a Box is on its machine.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub(crate) struct MachineInfo {
    /// The calling Box's id.
    #[serde(rename = "box")]
    pub(crate) box_id: i64,
    /// The machine's name.
    pub(crate) machine: String,
    /// The Box's slot on the machine.
    pub(crate) slot: u16,
    /// Add this to a repo's port for the Box's port on the machine: 10 times the slot.
    pub(crate) port_offset: u32,
    /// What the machine's operator set up for this Box, such as its own device.
    pub(crate) facts: Facts,
    /// rsync patterns every sync leaves alone; a sync's own `exclude` adds to them.
    pub(crate) sync_exclude: Vec<String>,
    /// Set when the Box's files on the machine were removed while it slept: sync again.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) notice: Option<String>,
}
