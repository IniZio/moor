//! Reads the events of a Box a test started.

use moor_core::BoxId;
use serde_json::Value;

use super::World;

/// The payloads of Box `id`'s events of `kind`.
pub(super) fn events(w: &World, id: BoxId, kind: &str) -> Vec<Value> {
    let store = w.cx.store().unwrap();
    let rows = store.newest_events_of(id, kind, 10).unwrap();
    rows.into_iter().map(|e| e.payload).collect()
}
