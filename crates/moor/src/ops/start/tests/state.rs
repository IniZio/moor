//! Reads of the Box lifecycle and primary Session state a start leaves on a row.

use moor_core::domain::box_state::BoxLifecycle;
use moor_core::domain::session_state::SessionState;

use crate::store::BoxRow;

pub(super) const WORKING: (BoxLifecycle, SessionState) =
    (BoxLifecycle::Ready, SessionState::Working);

pub(super) fn working(row: &BoxRow) -> bool {
    row.lifecycle == BoxLifecycle::Ready && !row.session.is_quiet()
}

pub(super) fn idle(row: &BoxRow) -> bool {
    row.lifecycle == BoxLifecycle::Ready && row.session.is_quiet()
}
