//! Resolve a request's [`BoxTarget`] to one Box row.

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::BoxTarget;

use crate::store::{BoxRow, Store, StoreError};

/// Which Box a request means: by name, which of the Boxes with that name; by id, whether a
/// closed Box counts.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Rule {
    /// `steer`: the newest open Box of the name; an id must name an open Box.
    Open,
    /// `close`: the newest open Box of the name, else the newest closed one; an id names its Box
    /// in any state.
    OpenElseLatest,
    /// `show` and the artifact requests: the newest Box of the name in any state; an id names its
    /// Box in any state.
    Latest,
}

/// The Box `target` means under `rule`; none when no Box matches.
///
/// # Errors
/// [`StoreError`] when a store read fails.
pub(crate) fn resolve(
    store: &Store,
    target: &BoxTarget,
    rule: Rule,
) -> Result<Option<BoxRow>, StoreError> {
    match (target, rule) {
        (BoxTarget::Id(id), Rule::Open) => Ok(store.box_row(*id)?.filter(is_open)),
        (BoxTarget::Id(id), Rule::OpenElseLatest | Rule::Latest) => store.box_row(*id),
        (BoxTarget::Name(name), Rule::Open) => store.open_box_by_name(name),
        (BoxTarget::Name(name), Rule::OpenElseLatest) => store
            .open_box_by_name(name)?
            .map_or_else(|| store.latest_box_by_name(name), |row| Ok(Some(row))),
        (BoxTarget::Name(name), Rule::Latest) => store.latest_box_by_name(name),
    }
}

/// Open as `Store::open_box_by_name` counts it: neither closed nor rejected.
const fn is_open(row: &BoxRow) -> bool {
    !matches!(row.lifecycle, BoxLifecycle::Closed | BoxLifecycle::Rejected)
}
