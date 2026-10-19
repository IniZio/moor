//! Reads of the Box lifecycle and primary Session state a start leaves on a row.

use moor_core::domain::box_state::BoxLifecycle;

use crate::store::TaskRow;

pub(super) fn working(row: &TaskRow) -> bool {
    row.lifecycle == BoxLifecycle::Ready && !row.session.is_quiet()
}

pub(super) fn idle(row: &TaskRow) -> bool {
    row.lifecycle == BoxLifecycle::Ready && row.session.is_quiet()
}
