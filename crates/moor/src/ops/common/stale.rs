//! Writes about hands a Box has since replaced.

use moor_core::Warning;

use crate::cx::Cx;
use crate::store::StoreError;

/// What `written` wrote; none when it was about hands the Box has since replaced. Nothing is left
/// to do for the VM it was about, so this warns once and the caller moves on, never retrying.
pub(crate) fn unless_stale<T>(
    cx: &Cx,
    written: Result<T, StoreError>,
) -> Result<Option<T>, StoreError> {
    match written {
        Ok(v) => Ok(Some(v)),
        Err(e @ StoreError::Stale { .. }) => {
            cx.warn(Warning {
                code: "stale_hands_write".to_owned(),
                message: e.to_string(),
            });
            Ok(None)
        }
        Err(e) => Err(e),
    }
}
