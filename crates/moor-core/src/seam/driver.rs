//! Driver seam: how moor gets, inspects and releases Boxes.
//!
//! Deviation from design 6.1: only the S3 methods exist here; exec, sync,
//! file transfer, sleep, tunnel and connect land with later slices.

use std::fmt;
use std::time::Duration;

use crate::name::{BoxHandle, BoxPath, ProviderId, TaskId};

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

/// How far a provider can restrict Box egress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EgressSupport {
    /// Always enforced.
    Enforced,
    /// Enforced when requested.
    OptIn,
    /// Not available.
    None,
}

/// Facts about a provider, as data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DriverCaps {
    /// Sleep style.
    pub sleep: SleepKind,
    /// Whether a sleeping Box can wake.
    pub wake: bool,
    /// Egress restriction support.
    pub egress: EgressSupport,
    /// Box-to-host tunnel support.
    pub reverse_tunnel: bool,
    /// Whether the Box reports a working directory.
    pub has_workdir: bool,
}

/// The egress policy a new Box should get.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EgressPlan {
    /// No restriction.
    Open,
    /// Only these hosts.
    Allow(Vec<String>),
}

/// Whether the provider applied the requested egress plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EgressApplied {
    /// The plan is in force.
    Enforced,
    /// The provider ignored the plan.
    NotEnforced,
}

/// A request for a new Box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcquireReq {
    /// Provider to ask.
    pub provider: ProviderId,
    /// Source for the per-Task Box name the adapter derives.
    pub task: TaskId,
    /// Requested egress.
    pub egress: EgressPlan,
    /// Optional size class.
    pub class: Option<String>,
}

/// A Box the Driver has handed over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Acquired {
    /// Opaque handle.
    pub handle: BoxHandle,
    /// Working directory.
    pub workdir: BoxPath,
    /// Egress outcome.
    pub egress: EgressApplied,
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

/// The only error shape Driver adapters return.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriverError {
    /// No such Box.
    NotFound,
    /// Slot limit reached.
    Capacity {
        /// Slot limit, when known.
        limit: Option<u32>,
        /// Who holds the slots, when known.
        holder: Option<String>,
    },
    /// Provider quota hit.
    Quota(String),
    /// Credentials rejected.
    Auth(String),
    /// The operation outlived its allowance.
    Timeout(Duration),
    /// Provider unreachable.
    Unavailable(String),
    /// Provider cannot do this.
    Unsupported(&'static str),
    /// Any other failure, summarised by the adapter.
    Failed {
        /// Moor-worded summary.
        summary: String,
    },
}

impl fmt::Display for DriverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => write!(f, "box not found"),
            Self::Capacity { limit, holder } => fmt_capacity(f, *limit, holder.as_deref()),
            Self::Quota(m) => write!(f, "provider quota reached: {m}"),
            Self::Auth(m) => write!(f, "provider authentication failed: {m}"),
            Self::Timeout(d) => write!(f, "box operation timed out after {}s", d.as_secs()),
            Self::Unavailable(m) => write!(f, "box provider unavailable: {m}"),
            Self::Unsupported(what) => write!(f, "not supported by this provider: {what}"),
            Self::Failed { summary } => write!(f, "box operation failed: {summary}"),
        }
    }
}

/// Writes the capacity message, naming limit and holder when known.
fn fmt_capacity(
    f: &mut fmt::Formatter<'_>,
    limit: Option<u32>,
    holder: Option<&str>,
) -> fmt::Result {
    write!(f, "box capacity reached")?;
    if let Some(n) = limit {
        write!(f, " (limit {n})")?;
    }
    if let Some(h) = holder {
        write!(f, ", held by {h}")?;
    }
    Ok(())
}

impl std::error::Error for DriverError {}

/// Boxes, behind whichever provider hub is wired.
pub trait Driver: Send + Sync {
    /// Reports what `provider` can do.
    ///
    /// # Errors
    /// Returns a [`DriverError`] when the provider is unknown or unreachable.
    fn caps(&self, provider: &ProviderId) -> Result<DriverCaps, DriverError>;

    /// Gets a new Box for a Task.
    ///
    /// # Errors
    /// Returns [`DriverError::Capacity`], `Quota`, `Auth` and the like.
    fn acquire(&self, req: &AcquireReq) -> Result<Acquired, DriverError>;

    /// Releases a Box; an already-gone Box is not an error.
    ///
    /// # Errors
    /// Returns a [`DriverError`] when the release fails.
    fn release(&self, handle: &BoxHandle) -> Result<Released, DriverError>;

    /// Lists the Boxes the Driver knows under one provider. Each listing's
    /// handle equals the handle `acquire` returned for that Box.
    ///
    /// # Errors
    /// Returns a [`DriverError`] when the provider cannot be queried.
    fn list(&self, provider: &ProviderId) -> Result<Vec<BoxListing>, DriverError>;

    /// Returns the Box's working directory.
    ///
    /// # Errors
    /// Returns [`DriverError::NotFound`] when the Box is gone.
    fn workdir(&self, handle: &BoxHandle) -> Result<BoxPath, DriverError>;
}

#[cfg(test)]
mod tests;
