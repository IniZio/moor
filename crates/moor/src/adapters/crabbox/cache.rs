//! The cache plan as crabbox reads it: `CRABBOX_CACHE_VOLUMES`, a JSON array, on warmup only.

use moor_client::link::lease::{Cache, CacheVolume};
use moor_core::cache::CachePlan;
use moor_core::seam::proc::ProcSpec;

/// The env key crabbox reads cache volumes from.
const KEY: &str = "CRABBOX_CACHE_VOLUMES";

/// The env key crabbox reads the Box's cache scope from.
const SCOPE_KEY: &str = "CRABBOX_CACHE_SCOPE";

/// The plan's volumes in crabbox's shape; none for a cold Box.
fn volumes(plan: &CachePlan) -> Vec<CacheVolume> {
    plan.volumes()
        .iter()
        .map(|v| CacheVolume {
            group: v.group.clone(),
            name: v.name.as_str().to_owned(),
            key: v.key.clone(),
            paths: v.paths.iter().map(|p| p.as_str().to_owned()).collect(),
            size_gb: v.size_gb.map_or(0, std::num::NonZeroU32::get),
            sealed: !v.seal().is_empty(),
        })
        .collect()
}

/// The `CRABBOX_CACHE_VOLUMES` value for `plan`; none for a cold Box.
pub(super) fn env_value(plan: &CachePlan) -> Option<String> {
    let wire = volumes(plan);
    if wire.is_empty() {
        return None;
    }
    serde_json::to_string(&wire).ok()
}

/// The plan as a pool host's warmup takes it; none for a cold Box.
pub(super) fn lease(plan: &CachePlan) -> Option<Cache> {
    let volumes = volumes(plan);
    (!volumes.is_empty()).then(|| Cache {
        volumes,
        scope: plan.scope().map(str::to_owned),
    })
}

/// `spec` with the plan's volumes and scope in its env, when there are any.
pub(super) fn with_volumes(spec: ProcSpec, plan: &CachePlan) -> ProcSpec {
    let Some(volumes) = env_value(plan) else {
        return spec;
    };
    let spec = spec.env(KEY, volumes);
    match plan.scope() {
        Some(scope) => spec.env(SCOPE_KEY, scope),
        None => spec,
    }
}

#[cfg(test)]
mod tests;
