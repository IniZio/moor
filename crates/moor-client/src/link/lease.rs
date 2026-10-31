//! A pool host's VM leases: what the runner may ask of them and what the host answers.
//!
//! An answer is data only. The host never sends ssh words or a command for the runner to run;
//! it reaches a VM through a dial, which the host carries itself.

use serde::{Deserialize, Serialize};

/// What to do with a lease.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Verb {
    /// Boot the VM, or find it booted.
    Warmup,
    /// How to sign in to the VM and which host keys it shows.
    Info,
    /// Pause the VM, keeping its disk and memory.
    Pause,
    /// Resume a paused VM.
    Resume,
    /// Delete the VM; one already gone counts as deleted.
    Stop,
    /// The pool's leases; it names no slug.
    List,
    /// Let a public key sign in to the VM; the request carries the key.
    Authorize,
}

/// The host's answer to a lease request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "answer")]
pub enum Answer {
    /// The verb took effect.
    Done,
    /// The host's provider cannot do the verb.
    Unsupported,
    /// The VM's sign-in.
    Info {
        /// The VM's login user.
        user: String,
        /// The VM's host public keys, each `type base64`.
        host_keys: Vec<String>,
    },
    /// The pool's `moor-<controller>-<id>` leases.
    Leases {
        /// Each lease.
        leases: Vec<Lease>,
    },
}

/// One lease in a list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lease {
    /// `moor-<controller>-<id>`.
    pub slug: String,
    /// What the provider says of it, such as `running` or `paused`.
    pub state: String,
}

/// The cache volumes a warmup mounts, as crabbox reads them from `CRABBOX_CACHE_VOLUMES` and
/// `CRABBOX_CACHE_SCOPE`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cache {
    /// The volumes, in crabbox's JSON shape.
    pub volumes: Vec<CacheVolume>,
    /// The Box's scope, 16 lowercase hex digits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

/// One volume in crabbox's `CacheVolumeConfig` shape, extended with `group`, `paths` and
/// `sealed`.
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
    /// The volume has a seal; the provider keeps it only with the seal's marker in each path.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub sealed: bool,
}
