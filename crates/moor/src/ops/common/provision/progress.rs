//! What the driver reports while it acquires a Box, kept as the Box's events.

use moor_core::domain::decision::{Decision, ExtraEvent};
use moor_core::domain::event::PostCloneStep;
use moor_core::seam::driver::{AcquireReq, Acquired, DriverError};
use moor_core::BoxId;

use crate::cx::Cx;
use crate::ops::common::setup::LINK_WAIT;
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
        match cx.driver().acquire_noting(req, &mut progress(store, id)) {
            Err(e) if goes_on_after(cx, req, &e) => take_over(cx, store, id, req),
            got => got,
        }
    })
}

/// Whether `e` cut off an acquire that goes on without its caller.
pub(in crate::ops::common::provision) fn goes_on_after(
    cx: &Cx,
    req: &AcquireReq,
    e: &DriverError,
) -> bool {
    matches!(e, DriverError::Transport { .. })
        && cx
            .driver()
            .acquire_outlives_caller(&cx.driver().planned_handle(req))
}

/// Takes over the acquire of Box `id` that a cut-off process left going, timed as an acquire.
pub(super) fn adopt_timed(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    req: &AcquireReq,
) -> Result<Acquired, DriverError> {
    timed_result(cx, store, id, PostCloneStep::Acquire, || {
        take_over(cx, store, id, req)
    })
}

/// Pause before asking a host out of reach again.
const RELINK_POLL: std::time::Duration = std::time::Duration::from_secs(1);

/// Takes over the acquire going on for Box `id`. A host out of reach is not the acquire failing, so
/// it is asked again until [`LINK_WAIT`], or until a drain leaves it to the next process.
pub(in crate::ops::common::provision) fn take_over(
    cx: &Cx,
    store: &Store,
    id: BoxId,
    req: &AcquireReq,
) -> Result<Acquired, DriverError> {
    let began = cx.clock().instant();
    loop {
        let got = cx.driver().adopt_noting(req, &mut progress(store, id));
        let waited = cx.clock().instant().saturating_duration_since(began);
        let stopped = cx.env().draining().is_cancelled() || cx.env().cancel().is_cancelled();
        let again = waited < LINK_WAIT && !stopped;
        match got {
            Err(DriverError::Transport { .. }) if again => cx.clock().sleep(RELINK_POLL),
            got => return got,
        }
    }
}

/// Records each note the driver hands over while it acquires Box `id`.
fn progress(store: &Store, id: BoxId) -> impl FnMut(&str) + '_ {
    move |text| {
        let text = text.to_owned();
        drop(store.apply(Decision::new(id).event(ExtraEvent::BoxProgress { text })));
    }
}
