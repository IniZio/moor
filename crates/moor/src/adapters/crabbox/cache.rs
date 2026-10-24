//! The cache plan as crabbox reads it: `CRABBOX_CACHE_VOLUMES`, a JSON array, on warmup only.

use moor_core::cache::{CachePath, CachePlan};
use moor_core::seam::proc::ProcSpec;
use serde::Serialize;

/// The env key crabbox reads cache volumes from.
const KEY: &str = "CRABBOX_CACHE_VOLUMES";

/// One volume in crabbox's `CacheVolumeConfig` shape, extended with `group`, `paths`
/// and `sealed`.
#[derive(Serialize)]
struct Wire<'a> {
    /// Scope id; the volumes of one group share one store.
    group: &'a str,
    /// Name, a subdirectory of the group's store.
    name: &'a str,
    /// `<group>-<name>`.
    key: &'a str,
    /// Box paths, `~/` for the login home.
    paths: Vec<&'a str>,
    /// Size hint; 0 is the adapter's default.
    #[serde(rename = "sizeGB")]
    size_gb: u32,
    /// The volume has a seal; the provider keeps it only with the seal's marker in each path.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    sealed: bool,
}

/// The `CRABBOX_CACHE_VOLUMES` value for `plan`; none for a cold Box.
pub(super) fn env_value(plan: &CachePlan) -> Option<String> {
    let wire: Vec<Wire<'_>> = plan
        .volumes()
        .iter()
        .map(|v| Wire {
            group: &v.group,
            name: v.name.as_str(),
            key: &v.key,
            paths: v.paths.iter().map(CachePath::as_str).collect(),
            size_gb: v.size_gb.map_or(0, std::num::NonZeroU32::get),
            sealed: !v.seal().is_empty(),
        })
        .collect();
    if wire.is_empty() {
        return None;
    }
    serde_json::to_string(&wire).ok()
}

/// `spec` with the plan's volumes in its env, when there are any.
pub(super) fn with_volumes(spec: ProcSpec, plan: &CachePlan) -> ProcSpec {
    match env_value(plan) {
        Some(v) => spec.env(KEY, v),
        None => spec,
    }
}

#[cfg(test)]
mod tests;
