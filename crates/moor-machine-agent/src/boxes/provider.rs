//! How a machine provides Boxes. The rest of the agent knows a Box only through this.

use std::fmt;
use std::future::Future;

use moor_client::link::lease::Cache;
use moor_core::seam::driver::Place;
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tokio::sync::mpsc;

use super::Slug;

/// What an open makes a Box from.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Spec {
    /// The machine class; none leaves it to the provider.
    pub(crate) class: Option<String>,
    /// The cache volumes the work place mounts.
    pub(crate) cache: Option<Cache>,
}

/// The environment the controller's Proxy plan gives a Box.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Env {
    /// Exported before every command, in both places.
    pub(crate) vars: Vec<(String, String)>,
    /// A CA certificate, PEM, both places trust.
    pub(crate) ca_pem: Option<String>,
}

/// Why a provider could not do what it was asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProvideError {
    /// This machine has no such Box.
    Missing,
    /// It tried and failed.
    Failed(String),
}

impl fmt::Display for ProvideError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => f.write_str("the Box does not exist on this machine"),
            Self::Failed(why) => f.write_str(why),
        }
    }
}

/// Where a provider says what it is doing while an open works, such as pulling an image; each
/// line reaches the controller as a `note`.
#[derive(Debug, Clone)]
pub(crate) struct Notes(mpsc::UnboundedSender<String>);

impl Notes {
    /// A handle, and the lines said through it.
    pub(crate) fn new() -> (Self, mpsc::UnboundedReceiver<String>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self(tx), rx)
    }

    /// Says `line`.
    pub(crate) fn say(&self, line: &str) {
        let _ = self.0.send(line.to_owned());
    }
}

/// How one place of a Box stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlaceState {
    /// Running.
    Up,
    /// Stopped or not there.
    Down,
}

/// One Box as the provider sees it now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Observed {
    /// The Box.
    pub(crate) slug: Slug,
    /// The agent place.
    pub(crate) agent: PlaceState,
    /// The work place.
    pub(crate) work: PlaceState,
    /// How many processes the work place's memory limit has killed.
    pub(crate) oom_kills: u32,
    /// MiB of the host the Box holds, when the provider can tell.
    pub(crate) mem_mib: Option<u64>,
}

/// What a provider promises about a Box: two places, a workdir that is the same path with the
/// same files in both, and an environment both places see. Nothing here names how it is made.
pub(crate) trait BoxProvider: Send + Sync + 'static {
    /// How it provides Boxes, for the offer; people read it and nothing branches on it.
    fn provides(&self) -> &'static str;

    /// Makes what is missing of `slug` and starts what is stopped; repeating it finds the same
    /// Box. The workdir.
    fn open(
        &self,
        slug: &Slug,
        spec: &Spec,
        note: &Notes,
    ) -> impl Future<Output = Result<String, ProvideError>> + Send;

    /// Starts both places, the agent's first.
    fn start(&self, slug: &Slug) -> impl Future<Output = Result<(), ProvideError>> + Send;

    /// Stops both places, the work place first, keeping everything else.
    fn stop(&self, slug: &Slug) -> impl Future<Output = Result<(), ProvideError>> + Send;

    /// Removes both places and what the Box owns; gone is fine.
    fn remove(&self, slug: &Slug) -> impl Future<Output = Result<(), ProvideError>> + Send;

    /// Records the work place as it is now, to start a later repair from.
    fn seal(&self, slug: &Slug) -> impl Future<Output = Result<(), ProvideError>> + Send;

    /// Makes `env` true in both places now and after every later start.
    fn apply_env(
        &self,
        slug: &Slug,
        env: &Env,
    ) -> impl Future<Output = Result<(), ProvideError>> + Send;

    /// Every Box on this machine, as each place stands now.
    fn observe(&self) -> impl Future<Output = Result<Vec<Observed>, ProvideError>> + Send;

    /// A command that runs one shell script, its last argument, in `place`; on a terminal when
    /// `tty`. It starts in the workdir.
    fn carrier(&self, slug: &Slug, place: Place, tty: bool) -> Command;

    /// A script for the agent place that carries its stdin and stdout to `port` on the Box's
    /// loopback.
    fn forward_script(&self, port: u16) -> String {
        format!("exec socat STDIO TCP:127.0.0.1:{port}")
    }
}
