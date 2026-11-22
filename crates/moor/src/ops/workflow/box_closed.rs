//! The run of a Box that was just closed, settled at once as the next tick would settle it, so
//! nobody sees it wait on a Box that is gone and its workflow can run again straight away.

use moor_core::BoxId;

use super::advance::settle_run;
use crate::cx::Cx;
use crate::store::{Store, StoreError};

/// Settles the open run of Box `box_id`, which was closed, when it has one. A closed Box steers
/// no agent and runs no check, so the settle starts neither.
pub(crate) fn settle_closed_box(cx: &Cx, store: &Store, box_id: BoxId) -> Result<(), StoreError> {
    let Some((workflow, run)) = store.running_run_of_box(box_id)? else {
        return Ok(());
    };
    let Some(w) = store.workflow_by_id(workflow)? else {
        return Ok(());
    };
    let now = cx.clock().now();
    settle_run(cx, store, (&w, &run), now, (&|_| {}, &mut Vec::new()))
}
