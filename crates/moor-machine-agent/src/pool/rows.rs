//! Reading the rows of `crabbox list --json`.

use moor_client::link::lease::Lease;
use moor_client::link::BoxMemory;
use moor_core::box_key::parse_pool_slug;

/// The `moor-<controller>-<id>` slug a list row names in `labels.slug`, `slug` or `name`, by provider.
pub(super) fn pool_slug(row: &serde_json::Value) -> Option<&str> {
    [
        row.pointer("/labels/slug"),
        row.get("slug"),
        row.get("name"),
    ]
    .into_iter()
    .flatten()
    .find_map(serde_json::Value::as_str)
    .filter(|s| parse_pool_slug(s).is_some())
}

/// The pool leases among `rows`, with their states.
pub(super) fn leases(rows: &[serde_json::Value]) -> Vec<Lease> {
    rows.iter()
        .filter_map(|l| {
            let state = [l.get("state"), l.get("status")]
                .into_iter()
                .flatten()
                .find_map(serde_json::Value::as_str)
                .unwrap_or("unknown");
            Some(Lease {
                slug: pool_slug(l)?.to_owned(),
                state: state.to_owned(),
            })
        })
        .collect()
}

/// The host memory of each pool lease among `rows` that reports it.
pub(super) fn held_memory(rows: &[serde_json::Value]) -> Vec<BoxMemory> {
    rows.iter()
        .filter_map(|l| {
            Some(BoxMemory {
                slug: pool_slug(l)?.to_owned(),
                host_memory_mib: l.get("hostMemoryMiB")?.as_u64()?,
            })
        })
        .collect()
}
