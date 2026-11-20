//! The cache plan as a machine's agent takes it in an open.

use moor_client::link::lease::{Cache, CacheVolume};
use moor_core::cache::CachePlan;

/// The plan's volumes and scope; none for a cold Box.
pub(super) fn wire(plan: &CachePlan) -> Option<Cache> {
    let volumes: Vec<CacheVolume> = plan
        .volumes()
        .iter()
        .map(|v| CacheVolume {
            group: v.group.clone(),
            name: v.name.as_str().to_owned(),
            key: v.key.clone(),
            paths: v.paths.iter().map(|p| p.as_str().to_owned()).collect(),
            size_gb: v.size_gb.map_or(0, std::num::NonZeroU32::get),
        })
        .collect();
    (!volumes.is_empty()).then(|| Cache {
        volumes,
        scope: plan.scope().map(str::to_owned),
    })
}

#[cfg(test)]
mod tests;
