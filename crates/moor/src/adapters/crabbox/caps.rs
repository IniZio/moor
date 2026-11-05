//! Provider facts, derived from the feature tokens in `crabbox providers --json --configured`,
//! and the machine class each moor size asks for.

use moor_core::name::ProviderId;
use moor_core::seam::driver::{DriverCaps, DriverError, SleepKind};
use serde::Deserialize;

/// One provider entry; unknown fields are ignored.
#[derive(Debug, Deserialize)]
struct Entry {
    /// Canonical provider name.
    provider: String,
    /// Other names the provider answers to.
    #[serde(default)]
    aliases: Vec<String>,
    /// Feature names.
    #[serde(default)]
    features: Vec<String>,
    /// Lifecycle capability names.
    #[serde(default)]
    lifecycle: Vec<String>,
    /// Where the provider runs, such as `local-vm` or `direct-cloud`.
    #[serde(default)]
    category: String,
}

impl Entry {
    /// Whether `name` is this provider's name or alias.
    fn answers_to(&self, name: &str) -> bool {
        self.provider == name || self.aliases.iter().any(|a| a == name)
    }

    /// Whether the entry lists `cap` as a feature or lifecycle capability.
    fn has(&self, cap: &str) -> bool {
        self.features
            .iter()
            .chain(&self.lifecycle)
            .any(|c| c == cap)
    }
}

/// Feature token: the Box sleeps on its own.
const NATIVE_SLEEP: &str = "native-sleep";
/// Feature token: a wake changes the host key.
const ROTATES_HOST_KEY: &str = "resume-rotates-host-key";
/// Feature token: the Box is reached over SSH, which carries the reverse tunnel.
const SSH: &str = "ssh";
/// Feature token: the provider keeps cache volumes across Boxes.
const CACHE_VOLUME: &str = "cache-volume";
/// Feature token: the provider saves checkpoints of a lease.
const CHECKPOINT: &str = "workspace-checkpoint";
/// Feature token: the provider forks new leases from a checkpoint.
const FORK: &str = "workspace-fork";

/// Facts for `entry`, read from its feature tokens.
fn derive(entry: &Entry) -> DriverCaps {
    let sleep = if entry.has(NATIVE_SLEEP) {
        SleepKind::Native
    } else if entry.has("pause-resume") {
        SleepKind::Suspend
    } else {
        SleepKind::None
    };
    DriverCaps {
        sleep,
        wake: sleep != SleepKind::None && !entry.has(ROTATES_HOST_KEY),
        reverse_tunnel: entry.has(SSH),
        has_workdir: true,
        cache_volumes: entry.has(CACHE_VOLUME),
        checkpoints: entry.has(CHECKPOINT) && entry.has(FORK),
    }
}

/// Derives the facts for `id` from the providers JSON.
pub(super) fn caps_from(providers_json: &str, id: &ProviderId) -> Result<DriverCaps, DriverError> {
    let entries: Vec<Entry> =
        serde_json::from_str(providers_json).map_err(|_| DriverError::Failed {
            summary: "provider lookup returned unreadable output".to_owned(),
        })?;
    entries
        .iter()
        .find(|e| e.answers_to(id.as_str()))
        .map(derive)
        .ok_or_else(|| DriverError::UnknownProvider(id.as_str().to_owned()))
}

/// Categories of providers that run on crabbox's own host, so keep checkpoints on its disk.
/// An external provider is a program on this host, such as `crabbox-cloudhypervisor`.
const ON_THIS_HOST: [&str; 5] = [
    "local-runtime",
    "local-vm",
    "local-sandbox",
    "self-hosted-virtualization",
    "external-provider",
];

/// Whether `id` runs on crabbox's own host, read from the providers JSON; false when unknown.
pub(super) fn on_this_host(providers_json: &str, id: &ProviderId) -> bool {
    let entries: Vec<Entry> = serde_json::from_str(providers_json).unwrap_or_default();
    entries
        .iter()
        .find(|e| e.answers_to(id.as_str()))
        .is_some_and(|e| ON_THIS_HOST.contains(&e.category.as_str()))
}

/// Maps a moor size to crabbox's machine class.
pub(super) fn size_of(size: &str) -> Result<&'static str, DriverError> {
    match size {
        "small" => Ok("standard"),
        "medium" => Ok("fast"),
        "large" => Ok("large"),
        "xlarge" => Ok("beast"),
        other => Err(DriverError::Failed {
            summary: format!("unknown box size {other:?}"),
        }),
    }
}

/// The `--class` flag for a Box's size; none for a Box with no size, so crabbox picks the class.
pub(super) fn class_args(size: Option<&str>) -> Result<Vec<&'static str>, DriverError> {
    Ok(size
        .map(size_of)
        .transpose()?
        .map_or_else(Vec::new, |class| vec!["--class", class]))
}

#[cfg(test)]
mod tests;
