//! The cache volumes a Box's setup mounts.

use serde::{Deserialize, Serialize};

/// The cache volumes a Box mounts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cache {
    /// The volumes.
    pub volumes: Vec<CacheVolume>,
    /// The Box's scope, 16 lowercase hex digits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

/// One cache volume.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CacheVolume {
    /// Scope id; the volumes of one group share one store.
    pub group: String,
    /// Name, a subdirectory of the group's store.
    pub name: String,
    /// `<group>-<name>`.
    pub key: String,
    /// Box paths, `~/` for the login home.
    pub paths: Vec<String>,
    /// Size hint; 0 is the adapter's default.
    #[serde(rename = "sizeGB", default)]
    pub size_gb: u32,
}
