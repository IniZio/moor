//! What an op that parks a Box wants it to end as, recorded before the park so a close that
//! races it wins, and given up once the park is over.

use moor_core::domain::decision::{Decision, WantChange, Wanted, WantedKind};
use moor_core::BoxId;

use crate::error::CommandError;
use crate::store::Store;

/// Runs `park` with `w` wanted of Box `id`, under the turn the caller holds. Returns `None`
/// without parking when the Box wants a close, which outranks a park and must not wait behind
/// one. What the park did not end is given up after it, so nothing stale is left wanted.
pub(crate) fn parking<T>(
    store: &Store,
    id: BoxId,
    w: Wanted,
    park: impl FnOnce() -> Result<T, CommandError>,
) -> Result<Option<T>, CommandError> {
    let kind = w.kind();
    store.apply(Decision::new(id).want(WantChange::Raise(w)))?;
    let held = store.box_row(id)?.and_then(|r| r.wanted);
    if held.is_some_and(|h| h.kind() == WantedKind::Close) {
        return Ok(None);
    }
    let out = park();
    store.apply(Decision::new(id).want(WantChange::Done(kind)))?;
    out.map(Some)
}
