//! The user machine a Box claims, as `show` reports it.

use crate::config::MachineKindName;

/// A user machine a Box holds: its slot and the ports forwarded, already moved by the slot.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct HeldMachineView {
    /// The machine's name.
    pub name: String,
    /// Whether the Box holds the machine or a VM on it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<MachineKindName>,
    /// The crabbox provider of a VM host.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vm_provider: Option<String>,
    /// The Box's slot on it.
    pub slot: u16,
    /// The forwarded ports, the same number on both ends.
    pub ports: Vec<u16>,
    /// Whether the Box holds the slot now; an asleep or waiting Box shows its last slot and ports.
    pub state: MachineHoldState,
    /// While waiting for a slot, the Box's place in line, counting from 1.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<u32>,
}

/// Whether a Box that claims a machine holds a slot on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MachineHoldState {
    /// The Box holds its slot and its ports are forwarded.
    Held,
    /// The Box sleeps and its slot is free for another Box.
    Asleep,
    /// The Box is awake but every slot on its machine is taken.
    Waiting,
}
