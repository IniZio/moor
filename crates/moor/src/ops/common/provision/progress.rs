//! What the driver reports while it acquires a Box, kept as the Box's events.

use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::domain::event::PostCloneStep;
use moor_core::seam::driver::{AcquireReq, Acquired, DriverError};
use moor_core::BoxId;

use crate::cx::Cx;
use crate::ops::common::step::timed_result;
use crate::store::Store;

/// Acquires the machine for Box `id`, recording each note the driver hands over and the time the
/// whole call took.
pub(super) fn acquire_timed(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    req: &AcquireReq,
) -> Result<Acquired, DriverError> {
    timed_result(cx, store, id, PostCloneStep::Acquire, || {
        cx.driver().acquire_noting(req, &mut progress(store, id))
    })
}

/// Records each note the driver hands over while it acquires Box `id`.
fn progress(store: &Store, id: BoxId) -> impl FnMut(&str) + '_ {
    move |text| {
        let text = text.to_owned();
        drop(store.apply(Decision::new(id).event(ExtraEvent::BoxProgress { text })));
    }
}
