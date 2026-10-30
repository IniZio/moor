//! Whether a machine is itself the Box's machine or hosts a VM for each Box.

use serde::{Deserialize, Serialize};

/// Longest crabbox provider name.
const PROVIDER_MAX: usize = 64;

/// What a Box claim on a machine gets.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum MachineKind {
    /// The machine itself.
    #[default]
    Physical,
    /// A VM leased on the machine through its crabbox provider adapter.
    VmHost {
        /// The crabbox provider that leases the VMs.
        provider: String,
    },
}

/// The kind as the wire and the headers spell it, apart from the provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MachineKindName {
    /// `physical`.
    Physical,
    /// `vm-host`.
    VmHost,
}

impl MachineKindName {
    /// The spelling used in headers, tables and the store.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Physical => "physical",
            Self::VmHost => "vm-host",
        }
    }
}

impl MachineKind {
    /// The kind without its provider.
    #[must_use]
    pub const fn name(&self) -> MachineKindName {
        match self {
            Self::Physical => MachineKindName::Physical,
            Self::VmHost { .. } => MachineKindName::VmHost,
        }
    }

    /// The crabbox provider of a VM host.
    #[must_use]
    pub fn provider(&self) -> Option<&str> {
        match self {
            Self::Physical => None,
            Self::VmHost { provider } => Some(provider),
        }
    }

    /// The kind a wire or stored `name` and `provider` pair says.
    ///
    /// # Errors
    /// When a VM host has no valid provider or a physical machine has one.
    pub fn from_parts(name: MachineKindName, provider: Option<&str>) -> Result<Self, String> {
        match (name, provider) {
            (MachineKindName::Physical, None) => Ok(Self::Physical),
            (MachineKindName::VmHost, Some(p)) => {
                check_vm_provider(p)?;
                Ok(Self::VmHost {
                    provider: p.to_owned(),
                })
            }
            (MachineKindName::Physical, Some(_)) => {
                Err("a physical machine has no VM provider".to_owned())
            }
            (MachineKindName::VmHost, None) => Err("a VM host needs a VM provider".to_owned()),
        }
    }
}

/// `[a-z][a-z0-9-]*` in at most 64 bytes, as crabbox names a provider.
///
/// # Errors
/// Why `provider` is not accepted, naming `--vm`.
pub(super) fn check_vm_provider(provider: &str) -> Result<(), String> {
    let ok = provider.len() <= PROVIDER_MAX
        && provider.starts_with(|c: char| c.is_ascii_lowercase())
        && provider
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if ok {
        Ok(())
    } else {
        Err(format!(
            "--vm provider `{provider}` must match [a-z][a-z0-9-]* in at most {PROVIDER_MAX} bytes"
        ))
    }
}
