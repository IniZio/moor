//! Request and result values of the Driver calls: acquire, release, list, sleep and mirror.

use crate::box_key::PoolSeat;
use crate::cache::CachePlan;
use crate::name::{BoxHandle, BoxId, BoxName, BoxPath, ProviderId};

/// A request for a new Box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcquireReq {
    /// Provider to ask.
    pub provider: ProviderId,
    /// Source for the per-Box name the adapter derives.
    pub box_id: BoxId,
    /// Box name the adapter embeds in the Box name.
    pub name: BoxName,
    /// Box size (`small`, `medium`, `large`, `xlarge`); none leaves the class to the provider.
    pub class: Option<String>,
    /// Random suffix that keeps the Box name unique.
    pub nonce: String,
    /// Cache volumes the Box gets; empty for a cold Box.
    pub cache: CachePlan,
    /// Where the Box lives, for a provider that names a host.
    pub pool_machine: Option<PoolSeat>,
}

/// A Box the Driver has handed over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Acquired {
    /// Opaque handle.
    pub handle: BoxHandle,
    /// Working directory.
    pub workdir: BoxPath,
}

/// Result of releasing a Box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Released {
    /// The Box was deleted.
    Deleted,
    /// The Box did not exist.
    AlreadyGone,
}

/// One Box as the Driver lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoxListing {
    /// Opaque handle.
    pub handle: BoxHandle,
    /// Provider, when known.
    pub provider: Option<ProviderId>,
    /// Whether the Box sleeps.
    pub asleep: bool,
}

/// Whether a sleep may save the Box's caches; a Box suspended at work could hold torn files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PausePublish {
    /// The provider may save the caches once the Box is suspended.
    Allowed,
    /// The provider saves nothing for this sleep.
    Denied,
}

/// What `Driver::sleep` did for a Box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SleepOutcome {
    /// The Driver suspended the Box.
    Slept,
    /// The provider sleeps the Box on its own; nothing was called.
    Native,
    /// The Box stays up; the reason is worded for people.
    CannotSleep(String),
}
