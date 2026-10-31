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
