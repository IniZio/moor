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

/// The memory of each pool lease among `rows` that reports what it holds or what it needs to
/// resume. A row that says only the latter is suspended, so it holds nothing.
pub(super) fn held_memory(rows: &[serde_json::Value]) -> Vec<BoxMemory> {
    let mib = |l: &serde_json::Value, key| l.get(key).and_then(serde_json::Value::as_u64);
    rows.iter()
        .filter_map(|l| {
            let held = mib(l, "hostMemoryMiB");
            let resume = mib(l, "resumeMemoryMiB");
            held.or(resume)?;
            Some(BoxMemory {
                slug: pool_slug(l)?.to_owned(),
                host_memory_mib: held.unwrap_or(0),
                resume_memory_mib: resume,
            })
        })
        .collect()
}
