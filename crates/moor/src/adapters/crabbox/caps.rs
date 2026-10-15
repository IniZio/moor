//! Provider facts: derived from the feature tokens in `crabbox providers --json`.

use moor_core::name::ProviderId;
use moor_core::seam::driver::{DriverCaps, DriverError, EgressSupport, SleepKind};
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
/// Feature token: the provider can apply an egress allow-list.
const EGRESS_ALLOW: &str = "egress-allow";
/// Feature token: the allow-list is fixed at Box creation, so enforcement is opt-in.
const EGRESS_CREATE_ONLY: &str = "egress-allow-create-only";
/// Feature token: a wake changes the host key.
const ROTATES_HOST_KEY: &str = "resume-rotates-host-key";
/// Feature token: the Box is reached over SSH, which carries the reverse tunnel.
const SSH: &str = "ssh";

/// Facts for `entry`, read from its feature tokens.
fn derive(entry: &Entry) -> DriverCaps {
    let sleep = if entry.has(NATIVE_SLEEP) {
        SleepKind::Native
    } else if entry.has("pause-resume") {
        SleepKind::Suspend
    } else {
        SleepKind::None
    };
    let egress = if !entry.has(EGRESS_ALLOW) {
        EgressSupport::None
    } else if entry.has(EGRESS_CREATE_ONLY) {
        EgressSupport::OptIn
    } else {
        EgressSupport::Enforced
    };
    DriverCaps {
        sleep,
        wake: sleep != SleepKind::None && !entry.has(ROTATES_HOST_KEY),
        egress,
        reverse_tunnel: entry.has(SSH),
        has_workdir: true,
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
        .ok_or(DriverError::Unsupported("unknown box provider"))
}

#[cfg(test)]
mod tests;
