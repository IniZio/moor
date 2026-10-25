//! The hook report moor cached for a Box.

use moor_core::activity::HookCache;
use moor_core::BoxId;

use super::warn;
use crate::cx::Cx;
use crate::error::CommandError;
use crate::store::{Store, StoreError};

/// The cached hook report of Box `id`; an undecodable row is dropped with a warning.
pub(super) fn stored_hook(
    cx: &Cx,
    store: &Store,
    id: BoxId,
) -> Result<Option<HookCache>, CommandError> {
    match store.hook(id) {
        Ok(c) => Ok(c),
        Err(StoreError::Invalid(why) | StoreError::Corrupt(why)) => {
            warn(cx, format!("ignored the stored hook report: {why}"));
            Ok(None)
        }
        Err(StoreError::Json(e)) => {
            warn(cx, format!("ignored the stored hook report: {e}"));
            Ok(None)
        }
        Err(e) => Err(e.into()),
    }
}
